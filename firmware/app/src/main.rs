#![no_std]
#![no_main]

mod console;
mod identity;
mod update;

use core::cell::RefCell;
use core::convert::Infallible;

use embassy_boot_rp::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig, State};
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::flash::{Blocking, Flash, WRITE_SIZE};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, DMA_CH2, PIO0, SPI0, USB};
use embassy_rp::pio::{self, Pio};
use embassy_rp::pio_programs::ws2812::{Grb, PioWs2812, PioWs2812Program};
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::usb;
use embassy_rp::watchdog::Watchdog;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::UsbDevice;
use embassy_usb::class::cdc_acm::{self, CdcAcmClass};
use panic_halt as _;
use smart_leds::RGB8;
use static_cell::StaticCell;
use tpm::SpiInterface;
use tpm::interface::spi::SpiError;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>, dma::InterruptHandler<DMA_CH2>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
});

/// Must match the bootloader's `FLASH_SIZE`.
const FLASH_SIZE: usize = 8 * 1024 * 1024;
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(8);
/// Change this along with the version so an update is visible at a glance.
const LED_COLOR: RGB8 = RGB8::new(0, 0, 32);

type Led = PioWs2812<'static, PIO0, 0, 1, Grb>;
/// The TPM9670 board, see the README for wiring.
pub type Tpm = tpm::Tpm<SpiInterface<Spi<'static, SPI0, Async>, Output<'static>>, Delay>;
pub type TpmError = tpm::Error<SpiError<spi::Error, Infallible>>;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // The bootloader leaves the watchdog running; take it over and feed it.
    let mut watchdog = Watchdog::new(p.WATCHDOG);
    watchdog.start(WATCHDOG_TIMEOUT);
    // Spawning only fails if a task is spawned twice (`SpawnError::Busy`), a bug that shows up on
    // the first boot, so the `unwrap()`s on these spawns are fine.
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
    static CDC_STATE: StaticCell<cdc_acm::State> = StaticCell::new();
    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        &mut [],
        CONTROL_BUF.init([0; 64]),
    );
    let mut serial = CdcAcmClass::new(&mut builder, CDC_STATE.init(cdc_acm::State::new()), 64);
    spawner.spawn(usb_task(builder.build()).unwrap());

    // TPM on SPI0: SCK GP18, MOSI GP19, MISO GP20, CS# GP10. Mode 0 at 1 MHz (the default) is
    // slow but safe over jumper wires; the SLB 9670 takes up to 43 MHz. Only the `tpm` command
    // talks to it, so a missing TPM can't get in the way of updates.
    let spi = Spi::new(
        p.SPI0,
        p.PIN_18,
        p.PIN_19,
        p.PIN_20,
        p.DMA_CH1,
        p.DMA_CH2,
        Irqs,
        spi::Config::default(),
    );
    let cs = Output::new(p.PIN_10, Level::High);
    let mut tpm = Tpm::new(SpiInterface::new(spi, cs), Delay);

    // DFU and BOOTLOADER_STATE partitions, from the shared linker layout.
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(
        Flash::<_, Blocking, FLASH_SIZE>::new_blocking(p.FLASH),
    ));
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
    let mut aligned = AlignedBuffer([0; WRITE_SIZE]);
    let mut updater = BlockingFirmwareUpdater::new(config, &mut aligned.0);

    // Until this image calls `mark_booted`, any reset (panic, watchdog, power loss) makes the
    // bootloader roll back to the previous image. Nobody is listening on the console this early,
    // so failures are kept in the note that `status` prints instead of being logged.
    let boot = match updater.get_state() {
        Ok(State::Swap) => {
            // Placeholder self-test: run for a while with USB and the LED up before trusting
            // this image. The console starts after it.
            Timer::after_secs(5).await;
            match updater.mark_booted() {
                Ok(()) => "update installed",
                Err(_) => "update not confirmed, next reset rolls back",
            }
        }
        // Confirm the old image again, or the next update is refused.
        Ok(State::Revert) => match updater.mark_booted() {
            Ok(()) => "last update failed, rolled back",
            Err(_) => "rolled back, not re-confirmed, updates refused",
        },
        Ok(_) => "normal",
        Err(_) => "couldn't read boot state",
    };

    loop {
        serial.wait_connection().await;
        // Returns when the host goes away.
        let _ = console::console(&mut serial, boot, &mut updater, &mut tpm).await;
    }
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
