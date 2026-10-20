//! Line-based console over USB serial.

use core::fmt::Write;

use cortex_m::peripheral::SCB;
use embassy_boot_rp::BlockingFirmwareUpdater;
use embassy_rp::peripherals::USB;
use embassy_rp::usb;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::EndpointError;
use embedded_storage::nor_flash::NorFlash;
use heapless::String;

use crate::{Tpm, identity, update};

pub type Serial = CdcAcmClass<'static, usb::Driver<'static, USB>>;

pub struct Disconnected;

impl From<EndpointError> for Disconnected {
    fn from(_: EndpointError) -> Self {
        Disconnected
    }
}

/// Line-based console: echoes input and runs a command on Enter. Most replies are written as one
/// packet, so they must fit in 64 bytes; longer ones go through [`write_all`].
///
/// This isn't the avengers, there's no banner on connect. `wait_connection()` returns when the host
/// enumerates the device, not when a terminal opens the port, so a banner arrives while `tio` is
/// still setting up the tty and gets lost. Now, the user just has to press `ENTER` to get to the
/// prompt.
pub async fn console<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    boot: &str,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    tpm: &mut Tpm,
) -> Result<(), Disconnected> {
    let mut packet = [0u8; 64];
    // Room for `sign` and its 64 hex digits.
    let mut line = [0u8; 128];
    let mut len = 0;
    loop {
        let n = serial.read_packet(&mut packet).await?;
        for &b in &packet[..n] {
            match b {
                // Terminals send CR for Enter.
                b'\r' => {
                    serial.write_packet(b"\r\n").await?;
                    run_command(serial, &line[..len], boot, updater, tpm).await?;
                    len = 0;
                    serial.write_packet(b"> ").await?;
                }
                // Backspace / DEL
                0x08 | 0x7f if len > 0 => {
                    len -= 1;
                    serial.write_packet(b"\x08 \x08").await?;
                }
                0x20..0x7f if len < line.len() => {
                    line[len] = b;
                    len += 1;
                    serial.write_packet(&[b]).await?;
                }
                _ => {}
            }
        }
    }
}

async fn run_command<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    line: &[u8],
    boot: &str,
    updater: &mut BlockingFirmwareUpdater<'_, DFU, STATE>,
    tpm: &mut Tpm,
) -> Result<(), Disconnected> {
    match line.trim_ascii() {
        b"" => {}
        b"help" => {
            write_all(
                serial,
                b"commands: help, version, status, tpm, pubkey, sign, reboot, update\r\n",
            )
            .await?
        }
        b"version" => {
            serial
                .write_packet(concat!(env!("CARGO_PKG_VERSION"), "\r\n").as_bytes())
                .await?
        }
        b"status" => {
            serial
                .write_packet(concat!("version ", env!("CARGO_PKG_VERSION"), "\r\n").as_bytes())
                .await?;
            serial.write_packet(b"boot    ").await?;
            serial.write_packet(boot.as_bytes()).await?;
            serial.write_packet(b"\r\n").await?;
        }
        b"tpm" => tpm_info(serial, tpm).await?,
        b"pubkey" => pubkey(serial, tpm).await?,
        cmd if cmd.starts_with(b"sign ") => sign(serial, tpm, &cmd[5..]).await?,
        b"reboot" => {
            serial.write_packet(b"rebooting\r\n").await?;
            reset().await;
        }
        // Normally sent by `tools/upload.py`, not typed.
        b"update" => match update::receive(serial, updater).await {
            Ok(()) => {
                serial.write_packet(b"OK\r\n").await?;
                reset().await;
            }
            Err(update::Error::Disconnected) => return Err(Disconnected),
            Err(e) => {
                serial.write_packet(b"ERR ").await?;
                serial.write_packet(e.message().as_bytes()).await?;
                serial.write_packet(b"\r\n").await?;
                // Drop whatever the host still sends, so it isn't read as typed input.
                let mut packet = [0u8; 64];
                while let Ok(Ok(_)) =
                    with_timeout(Duration::from_millis(100), serial.read_packet(&mut packet)).await
                {
                }
            }
        },
        _ => {
            serial
                .write_packet(b"unknown command, try 'help'\r\n")
                .await?
        }
    }
    Ok(())
}

/// TCG vendor ID of Infineon, who make the SLB 9670.
const INFINEON: u16 = 0x15D1;

/// Prints the TPM's IDs, takes locality 0, and starts the TPM, which also shows whether it runs
/// TPM 2.0 firmware. A wrong vendor usually means wiring: MISO floats when the TPM isn't driving
/// it, so the ID reads as noise.
async fn tpm_info(serial: &mut Serial, tpm: &mut Tpm) -> Result<(), Disconnected> {
    let mut reply = String::<64>::new();
    // Every reply fits, so `write!` can't fail.
    let _ = match tpm.id().await {
        Ok(id) if id.vid == INFINEON => write!(
            reply,
            "Infineon TPM, did {:04x} rid {:02x}\r\n",
            id.did, id.rid
        ),
        Ok(id) => write!(
            reply,
            "ERR unexpected vendor {:04x}, check wiring\r\n",
            id.vid
        ),
        Err(e) => write!(reply, "ERR {e}\r\n"),
    };
    serial.write_packet(reply.as_bytes()).await?;

    reply.clear();
    let _ = match tpm.request_locality().await {
        Ok(()) => write!(reply, "locality 0 active\r\n"),
        Err(e) => write!(reply, "ERR {e}\r\n"),
    };
    serial.write_packet(reply.as_bytes()).await?;

    reply.clear();
    let _ = match tpm.startup().await {
        Ok(()) => write!(reply, "TPM 2.0, started\r\n"),
        Err(e) => write!(reply, "ERR {e}\r\n"),
    };
    serial.write_packet(reply.as_bytes()).await?;
    Ok(())
}

/// Prints the identity key as a SEC1 uncompressed point in hex, creating it on first use.
async fn pubkey(serial: &mut Serial, tpm: &mut Tpm) -> Result<(), Disconnected> {
    // 65 bytes of key in hex, plus CR LF.
    let mut reply = String::<132>::new();
    // Every reply fits, so `write!` can't fail.
    match identity::public_key(tpm).await {
        Ok((key, created)) => {
            if created {
                serial.write_packet(b"created identity key\r\n").await?;
            }
            push_hex(&mut reply, &key.to_sec1());
            let _ = write!(reply, "\r\n");
        }
        Err(e) => {
            let _ = write!(reply, "ERR {e}\r\n");
        }
    }
    write_all(serial, reply.as_bytes()).await
}

/// `sign <nonce>`: signs a challenge with the identity key and prints the signature, `r || s` in
/// hex. The nonce is 64 hex digits; see [`identity`] for what gets signed.
async fn sign(serial: &mut Serial, tpm: &mut Tpm, nonce: &[u8]) -> Result<(), Disconnected> {
    // 64 bytes of signature in hex, plus CR LF.
    let mut reply = String::<130>::new();
    // Every reply fits, so `write!` can't fail.
    match parse_hex::<{ identity::NONCE_LEN }>(nonce.trim_ascii()) {
        None => {
            let _ = write!(reply, "ERR nonce must be 64 hex digits\r\n");
        }
        Some(nonce) => match identity::sign_challenge(tpm, &nonce).await {
            Ok(signature) => {
                push_hex(&mut reply, &signature.r);
                push_hex(&mut reply, &signature.s);
                let _ = write!(reply, "\r\n");
            }
            Err(e) => {
                let _ = write!(reply, "ERR {e}\r\n");
            }
        },
    }
    write_all(serial, reply.as_bytes()).await
}

/// Appends `bytes` in lowercase hex. The caller sizes `out` for it.
fn push_hex<const N: usize>(out: &mut String<N>, bytes: &[u8]) {
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
}

/// Exactly `2 * N` hex digits, either case.
fn parse_hex<const N: usize>(hex: &[u8]) -> Option<[u8; N]> {
    if hex.len() != 2 * N {
        return None;
    }
    let mut out = [0; N];
    for (byte, pair) in out.iter_mut().zip(hex.as_chunks::<2>().0) {
        let pair = core::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// Writes `data` in as many packets as it takes. A full last packet is followed by an empty one,
/// which is how USB marks the end of a transfer.
async fn write_all(serial: &mut Serial, data: &[u8]) -> Result<(), Disconnected> {
    let size = usize::from(serial.max_packet_size());
    for packet in data.chunks(size) {
        serial.write_packet(packet).await?;
    }
    if data.len().is_multiple_of(size) {
        serial.write_packet(&[]).await?;
    }
    Ok(())
}

async fn reset() -> ! {
    // Let the last reply reach the host before USB goes away.
    Timer::after_millis(100).await;
    SCB::sys_reset();
}
