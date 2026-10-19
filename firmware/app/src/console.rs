//! Line-based console over USB serial.

use cortex_m::peripheral::SCB;
use embassy_rp::peripherals::USB;
use embassy_rp::usb;
use embassy_time::Timer;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::EndpointError;

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
pub async fn console(serial: &mut Serial, boot: &str) -> Result<(), Disconnected> {
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
                    run_command(serial, &line[..len], boot).await?;
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

async fn run_command(serial: &mut Serial, line: &[u8], boot: &str) -> Result<(), Disconnected> {
    match line.trim_ascii() {
        b"" => {}
        b"help" => {
            serial
                .write_packet(b"commands: help, version, status, reboot\r\n")
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
        b"reboot" => {
            serial.write_packet(b"rebooting\r\n").await?;
            // Let the message reach the host before USB goes away.
            Timer::after_millis(100).await;
            SCB::sys_reset();
        }
        _ => {
            serial
                .write_packet(b"unknown command, try 'help'\r\n")
                .await?
        }
    }
    Ok(())
}
