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
//! Any step can fail with `ERR <reason>\r\n` instead. The host waits for each `A` before sending
//! more, so the device never receives more than it asked for.

use embassy_boot::FirmwareUpdaterError;
use embassy_boot_rp::{BlockingFirmwareUpdater, State};
use embassy_time::{Duration, with_timeout};
use embassy_usb::driver::EndpointError;
use embedded_storage::nor_flash::NorFlash;

use crate::console::Serial;

/// One flash erase sector, so each block is one erase + write.
const BLOCK_SIZE: usize = 4096;
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
    Flash,
    Signature,
}

impl From<EndpointError> for Error {
    fn from(_: EndpointError) -> Self {
        Error::Disconnected
    }
}

impl Error {
    pub fn message(&self) -> &'static str {
        match self {
            Error::Disconnected => "host disconnected",
            Error::NotConfirmed => "running image is not confirmed, see 'status'",
            Error::Timeout => "timed out waiting for data",
            Error::BadLength => "length must be n * 4096 and fit in ACTIVE",
            Error::TooMuchData => "got more data than asked for",
            Error::Flash => "flash write failed",
            Error::Signature => "signature check failed",
        }
    }
}

/// Run the whole update. On `Ok` the image is staged and marked, and the caller should reset.
pub async fn receive<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
) -> Result<(), Error> {
    // `write_firmware` refuses too, but this fails before the host sends anything.
    if !matches!(updater.get_state(), Ok(State::Boot)) {
        return Err(Error::NotConfirmed);
    }
    serial.write_packet(b"READY\r\n").await?;

    let mut len = [0u8; 4];
    read_exact(serial, &mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || !len.is_multiple_of(BLOCK_SIZE) || len > active_size() {
        return Err(Error::BadLength);
    }
    serial.write_packet(b"A").await?;

    let mut block = [0u8; BLOCK_SIZE];
    for offset in (0..len).step_by(BLOCK_SIZE) {
        read_exact(serial, &mut block).await?;
        // Erases the sector before writing it.
        updater
            .write_firmware(offset, &block)
            .map_err(|_| Error::Flash)?;
        serial.write_packet(b"A").await?;
    }

    let mut signature = [0u8; 64];
    read_exact(serial, &mut signature).await?;
    // Blocks the executor (and the watchdog feeder) while it hashes the image.
    updater
        .verify_and_mark_updated(UPDATE_KEY, &signature, len as u32)
        .map_err(|e| match e {
            FirmwareUpdaterError::Signature(_) => Error::Signature,
            _ => Error::Flash,
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
