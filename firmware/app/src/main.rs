#![no_std]
#![no_main]

use cortex_m::peripheral::SCB;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::{self, Pio};
use embassy_rp::pio_programs::ws2812::{Grb, PioWs2812, PioWs2812Program};
use embassy_rp::usb;
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Duration, Timer};
use embassy_usb::UsbDevice;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;
use panic_halt as _;
use smart_leds::RGB8;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
});

const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(8);
/// Change this along with the version so an update is visible at a glance.
const LED_COLOR: RGB8 = RGB8::new(0, 0, 32);

type Led = PioWs2812<'static, PIO0, 0, 1, Grb>;
type Serial = CdcAcmClass<'static, usb::Driver<'static, USB>>;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // The bootloader leaves the watchdog running; take it over and feed it.
    let mut watchdog = Watchdog::new(p.WATCHDOG);
    watchdog.start(WATCHDOG_TIMEOUT);
    spawner.spawn(watchdog_task(watchdog).unwrap());

    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);
    let program = PioWs2812Program::new(&mut common);
    // The KB2040 has no plain LED, only a NeoPixel on GPIO17.
    let led = PioWs2812::new(&mut common, sm0, p.DMA_CH0, Irqs, p.PIN_17, &program);
    spawner.spawn(led_task(led).unwrap());

    // USB serial console, set up as in embassy's `examples/rp/src/bin/usb_serial.rs`.
    let driver = usb::Driver::new(p.USB, Irqs);
    let mut config = embassy_usb::Config::new(0xc0de, 0xcafe); // test VID/PID from embassy's examples
    config.manufacturer = Some("kb2040-fw-update");
    config.product = Some("KB2040 console");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static CDC_STATE: StaticCell<State> = StaticCell::new();
    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        &mut [],
        CONTROL_BUF.init([0; 64]),
    );
    let mut serial = CdcAcmClass::new(&mut builder, CDC_STATE.init(State::new()), 64);
    spawner.spawn(usb_task(builder.build()).unwrap());

    loop {
        serial.wait_connection().await;
        // Returns when the host goes away.
        let _ = console(&mut serial).await;
    }
}

struct Disconnected;

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
async fn console(serial: &mut Serial) -> Result<(), Disconnected> {
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
                    run_command(serial, &line[..len]).await?;
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

async fn run_command(serial: &mut Serial, line: &[u8]) -> Result<(), Disconnected> {
    match line.trim_ascii() {
        b"" => {}
        b"help" => {
            serial
                .write_packet(b"commands: help, version, reboot\r\n")
                .await?
        }
        b"version" => {
            serial
                .write_packet(concat!(env!("CARGO_PKG_VERSION"), "\r\n").as_bytes())
                .await?
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

#[embassy_executor::task]
async fn usb_task(mut usb: UsbDevice<'static, usb::Driver<'static, USB>>) -> ! {
    usb.run().await
}

/// Feeds the watchdog. A panic (`panic-halt` spins) or a busy loop starves this task, so the
/// watchdog still resets a stuck board.
#[embassy_executor::task]
async fn watchdog_task(mut watchdog: Watchdog) -> ! {
    loop {
        watchdog.feed(WATCHDOG_TIMEOUT);
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::task]
async fn led_task(mut led: Led) -> ! {
    loop {
        led.write(&[LED_COLOR]).await;
        Timer::after_secs(1).await;
        led.write(&[RGB8::default()]).await;
        Timer::after_secs(1).await;
    }
}
