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

use crate::{Tpm, update};

pub type Serial = CdcAcmClass<'static, usb::Driver<'static, USB>>;

pub struct Disconnected;

impl From<EndpointError> for Disconnected {
    fn from(_: EndpointError) -> Self {
        Disconnected
    }
}

/// Line-based console: echoes input and runs a command on Enter. Every reply is written as one
/// packet, so it must fit in 64 bytes.
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
    let mut line = [0u8; 64];
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
            serial
                .write_packet(b"commands: help, version, status, tpm, reboot, update\r\n")
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

/// Prints the TPM's IDs, then takes locality 0. A wrong vendor usually means wiring: MISO floats
/// when the TPM isn't driving it, so the ID reads as noise.
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
    Ok(())
}

async fn reset() -> ! {
    // Let the last reply reach the host before USB goes away.
    Timer::after_millis(100).await;
    SCB::sys_reset();
}
