//! Line-based console over USB serial.

use core::fmt::{self, Write};

use cortex_m::peripheral::SCB;
use embassy_boot_rp::BlockingFirmwareUpdater;
use embassy_rp::peripherals::USB;
use embassy_rp::usb;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::EndpointError;
use embedded_storage::nor_flash::NorFlash;
use heapless::String;
use tpm::KeyKind;

use crate::{Tpm, update};

pub type Serial = CdcAcmClass<'static, usb::Driver<'static, USB>>;

pub struct Disconnected;

impl From<EndpointError> for Disconnected {
    fn from(_: EndpointError) -> Self {
        Disconnected
    }
}

/// What the commands work on.
pub struct Context<'a, DFU: NorFlash, STATE: NorFlash> {
    /// How this boot went, for `status`.
    pub boot: &'static str,
    pub updater: BlockingFirmwareUpdater<'a, DFU, STATE>,
    pub tpm: Tpm,
}

/// Appends formatted text to an [`Out`], like `write!`.
macro_rules! reply {
    ($out:expr, $($arg:tt)*) => {
        $out.print(format_args!($($arg)*))
    };
}

// After `reply!`, so the command modules can use it.
mod tpm_commands;

/// A command's reply. It's collected here, then [`flush`](Out::flush)ed in as many packets as it
/// takes. Sized for the longest reply (`pubkey`'s); anything past that is dropped.
struct Out(String<256>);

impl Out {
    fn new() -> Self {
        Self(String::new())
    }

    fn print(&mut self, args: fmt::Arguments<'_>) {
        let _ = self.0.write_fmt(args);
    }

    /// Appends `bytes` in lowercase hex.
    fn hex(&mut self, bytes: &[u8]) {
        for b in bytes {
            reply!(self, "{b:02x}");
        }
    }

    async fn flush(&mut self, serial: &mut Serial) -> Result<(), Disconnected> {
        if !self.0.is_empty() {
            write_all(serial, self.0.as_bytes()).await?;
            self.0.clear();
        }
        Ok(())
    }
}

/// Line-based console: echoes input and runs a command on Enter. Each command's reply is sent
/// once the command is done.
///
/// This isn't the avengers, there's no banner on connect. `wait_connection()` returns when the host
/// enumerates the device, not when a terminal opens the port, so a banner arrives while `tio` is
/// still setting up the tty and gets lost. Now, the user just has to press `ENTER` to get to the
/// prompt.
pub async fn console<DFU: NorFlash, STATE: NorFlash>(
    serial: &mut Serial,
    ctx: &mut Context<'_, DFU, STATE>,
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
                    run_command(serial, &line[..len], ctx).await?;
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
    ctx: &mut Context<'_, DFU, STATE>,
) -> Result<(), Disconnected> {
    let mut out = Out::new();
    match line.trim_ascii() {
        b"" => {}
        b"help" => reply!(
            out,
            "commands: help, version, status, tpm, pubkey [decrypt], sign, reboot, update\r\n"
        ),
        b"version" => reply!(out, "{}\r\n", env!("CARGO_PKG_VERSION")),
        b"status" => reply!(
            out,
            "version {}\r\nboot    {}\r\n",
            env!("CARGO_PKG_VERSION"),
            ctx.boot
        ),
        b"tpm" => tpm_commands::info(&mut out, &mut ctx.tpm).await,
        b"pubkey" => tpm_commands::pubkey(&mut out, &mut ctx.tpm, KeyKind::Signing).await,
        b"pubkey decrypt" => tpm_commands::pubkey(&mut out, &mut ctx.tpm, KeyKind::Decrypt).await,
        cmd if cmd.starts_with(b"sign ") => {
            tpm_commands::sign(&mut out, &mut ctx.tpm, &cmd[5..]).await
        }
        b"reboot" => {
            reply!(out, "rebooting\r\n");
            out.flush(serial).await?;
            reset().await;
        }
        // Normally sent by `tools/upload.py`, not typed.
        b"update" => match update::receive(serial, &mut ctx.updater).await {
            Ok(()) => {
                reply!(out, "OK\r\n");
                out.flush(serial).await?;
                reset().await;
            }
            Err(update::Error::Disconnected) => return Err(Disconnected),
            Err(e) => {
                reply!(out, "ERR {}\r\n", e.message());
                out.flush(serial).await?;
                // Drop whatever the host still sends, so it isn't read as typed input.
                let mut packet = [0u8; 64];
                while let Ok(Ok(_)) =
                    with_timeout(Duration::from_millis(100), serial.read_packet(&mut packet)).await
                {
                }
            }
        },
        _ => reply!(out, "unknown command, try 'help'\r\n"),
    }
    out.flush(serial).await
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
