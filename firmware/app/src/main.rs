#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{self, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_time::Timer;
use panic_halt as _;
use smart_leds::RGB8;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let Pio { mut common, sm0, .. } = Pio::new(p.PIO0, Irqs);
    let program = PioWs2812Program::new(&mut common);
    // The KB2040 has no plain LED, only a NeoPixel on GPIO17.
    let mut led = PioWs2812::new(&mut common, sm0, p.DMA_CH0, Irqs, p.PIN_17, &program);

    loop {
        led.write(&[RGB8::new(0, 0, 32)]).await;
        Timer::after_secs(1).await;
        led.write(&[RGB8::default()]).await;
        Timer::after_secs(1).await;
    }
}
