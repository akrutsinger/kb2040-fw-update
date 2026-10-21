//! Receive a signed app image over the console and stage it in DFU for the bootloader.
//!
//! Protocol, after the console reads `update` (integers are little-endian):
//!
//! 1. Device: `READY\r\n`
//! 2. Host: `u32` image length, a non-zero multiple of [`BLOCK_SIZE`] that fits in ACTIVE.
//!    Device: `A`
//! 3. Host: one [`BLOCK_SIZE`] block. Device writes it to DFU, then answers `A`. Repeat.
//! 4. Host: the 64-byte Ed25519 signature of `sha512(image)`, made with [`UPDATE_KEY`]'s private
//!    half (`tools/sign.py`). Device reads the image back from DFU, checks the signature, and only
//!    then marks it. Device: `OK\r\n`, then resets so the bootloader swaps the image in.
//!
//! `update encrypted` is the same, for a release from the update server (`tools/bridge.py`):
//!
//! - After `READY`, the host first sends the ticket's length (1 byte). Device: `A`. Then `enc`
//!   (65 bytes) and the ticket itself (see [`ticket`]). Device: `A` once it has opened it.
//! - Everything after that (blocks and signature) is encrypted with AES-128-CTR, one keystream
//!   across all of it. The device decrypts each block before writing it, so DFU holds the
//!   plaintext image, and the signature check is the same.
//!
//! Any step can fail with `ERR <reason>\r\n` instead. The host waits for each `A` before sending
//! more, so the device never receives more than it asked for.

use core::fmt;

use aes::Aes128;
use ctr::Ctr128BE;
use ctr::cipher::{KeyIvInit, StreamCipher};
use embassy_boot::FirmwareUpdaterError;
use embassy_boot_rp::{BlockingFirmwareUpdater, State};
use embassy_time::{Duration, with_timeout};
use embassy_usb::driver::EndpointError;
use embedded_storage::nor_flash::NorFlash;

use crate::console::Serial;
use crate::identity::NONCE_LEN;
use crate::{Tpm, ticket};

/// One flash erase sector, so each block is one erase + write.
const BLOCK_SIZE: usize = 4096;
const SIGNATURE_LEN: usize = 64;
/// How long to wait for the host before giving up.
const RX_TIMEOUT: Duration = Duration::from_secs(5);
/// Public half of the update signing key, extracted from `keys/update.pub` by `build.rs`.
static UPDATE_KEY: &[u8; 32] = include_bytes!(concat!(env!("OUT_DIR"), "/update.pub"));

pub enum Error {
    Disconnected,
    NotConfirmed,
    Timeout,
    BadLength,
    TooMuchData,
    FlashWrite,
    FlashVerify,
    Signature,
    /// `update encrypted` without a `request` since boot, so there's no nonce to check.
    NoRequest,
    Ticket(ticket::Error),
    /// The image length doesn't match the size in the ticket.
    SizeMismatch,
}

impl From<EndpointError> for Error {
    fn from(_: EndpointError) -> Self {
        Error::Disconnected
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Disconnected => "host disconnected",
            Error::NotConfirmed => "running image is not confirmed, see 'status'",
            Error::Timeout => "timed out waiting for data",
            Error::BadLength => "length must be n * 4096 and fit in ACTIVE",
            Error::TooMuchData => "got more data than asked for",
            Error::FlashWrite => "writing the image to flash failed",
            Error::FlashVerify => "flash error while verifying the image",
            Error::Signature => "signature check failed",
            Error::NoRequest => "no update request since boot, run 'request' first",
            Error::Ticket(ticket::Error::Tpm(e)) => return write!(f, "opening the ticket: {e}"),
            Error::Ticket(ticket::Error::BadEnc) => "ticket's enc isn't a P-256 point",
            Error::Ticket(ticket::Error::Open) => "ticket doesn't open, is it for this board?",
            Error::Ticket(ticket::Error::Format) => "ticket too short",
            Error::Ticket(ticket::Error::Nonce) => "ticket answers a different request",
            Error::SizeMismatch => "image length doesn't match the ticket",
        })
    }
}

/// Run the whole update. On `Ok` the image is staged and marked, and the caller should reset.
pub async fn receive<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
) -> Result<(), Error> {
    ready(serial, updater).await?;
    stage(serial, updater, None).await
}

/// [`receive`] for `update encrypted`. `nonce` is the one in this board's last update request.
pub async fn receive_encrypted<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    tpm: &mut Tpm,
    nonce: Option<[u8; NONCE_LEN]>,
) -> Result<(), Error> {
    let nonce = nonce.ok_or(Error::NoRequest)?;
    ready(serial, updater).await?;

    // The length on its own first, so the next read knows how much to expect and never gets a
    // packet with more than it asked for.
    let mut len = [0];
    read_exact(serial, &mut len).await?;
    serial.write_packet(b"A").await?;
    let mut header = [0; hpke::ENC_LEN + u8::MAX as usize];
    let header = &mut header[..hpke::ENC_LEN + usize::from(len[0])];
    read_exact(serial, header).await?;
    let (enc, sealed) = header.split_at_mut(hpke::ENC_LEN);
    let enc = (&*enc).try_into().unwrap();
    let ticket = ticket::open(tpm, enc, sealed, &nonce)
        .await
        .map_err(Error::Ticket)?;
    serial.write_packet(b"A").await?;

    let mut cipher = Ctr128BE::<Aes128>::new(&ticket.key.into(), &ticket.iv.into());
    stage(serial, updater, Some((&mut cipher, ticket.size))).await
}

/// Fails before the host sends anything if the update can't work; `write_firmware` would refuse
/// too, but only after the first block.
async fn ready<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
) -> Result<(), Error> {
    if !matches!(updater.get_state(), Ok(State::Boot)) {
        return Err(Error::NotConfirmed);
    }
    serial.write_packet(b"READY\r\n").await?;
    Ok(())
}

/// Steps 2 to 4: length, blocks, signature. With `cipher`, everything is decrypted on arrival,
/// and the length must match the release `size` from the ticket.
async fn stage<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    mut cipher: Option<(&mut Ctr128BE<Aes128>, u32)>,
) -> Result<(), Error> {
    let mut len = [0u8; 4];
    read_exact(serial, &mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || !len.is_multiple_of(BLOCK_SIZE) || len > active_size() {
        return Err(Error::BadLength);
    }
    if let Some((_, size)) = &cipher
        && len + SIGNATURE_LEN != *size as usize
    {
        return Err(Error::SizeMismatch);
    }
    serial.write_packet(b"A").await?;

    let mut block = [0u8; BLOCK_SIZE];
    for offset in (0..len).step_by(BLOCK_SIZE) {
        read_exact(serial, &mut block).await?;
        if let Some((cipher, _)) = &mut cipher {
            cipher.apply_keystream(&mut block);
        }
        // Erases the sector before writing it.
        updater
            .write_firmware(offset, &block)
            .map_err(|e| match e {
                FirmwareUpdaterError::BadState => Error::NotConfirmed,
                _ => Error::FlashWrite,
            })?;
        serial.write_packet(b"A").await?;
    }

    let mut signature = [0u8; SIGNATURE_LEN];
    read_exact(serial, &mut signature).await?;
    if let Some((cipher, _)) = &mut cipher {
        cipher.apply_keystream(&mut signature);
    }
    // Blocks the executor (and the watchdog feeder) while it hashes the image.
    updater
        .verify_and_mark_updated(UPDATE_KEY, &signature, len as u32)
        .map_err(|e| match e {
            FirmwareUpdaterError::Signature(_) => Error::Signature,
            FirmwareUpdaterError::BadState => Error::NotConfirmed,
            // Reading the image back, or writing the mark.
            FirmwareUpdaterError::Flash(_) => Error::FlashVerify,
        })
}

/// Fill `buf` from as many packets as it takes.
async fn read_exact(serial: &mut Serial, buf: &mut [u8]) -> Result<(), Error> {
    let mut packet = [0u8; 64];
    let mut filled = 0;
    while filled < buf.len() {
        let n = with_timeout(RX_TIMEOUT, serial.read_packet(&mut packet))
            .await
            .map_err(|_| Error::Timeout)??;
        let dst = buf.get_mut(filled..filled + n).ok_or(Error::TooMuchData)?;
        dst.copy_from_slice(&packet[..n]);
        filled += n;
    }
    Ok(())
}

/// Size of the ACTIVE partition, from the shared linker layout. Not the DFU size: DFU is one
/// page bigger for the swap, but an image has to fit where it will run.
fn active_size() -> usize {
    unsafe extern "C" {
        static __bootloader_active_start: u8;
        static __bootloader_active_end: u8;
    }
    // Only the symbols' addresses mean anything, never their contents.
    (&raw const __bootloader_active_end) as usize - (&raw const __bootloader_active_start) as usize
}
