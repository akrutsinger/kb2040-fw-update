//! embassy-boot-rp bootloader, based on embassy's `examples/boot/bootloader/rp`.

#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
use embassy_boot_rp::{BootLoader, BootLoaderConfig, WatchdogFlash};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Duration;

/// Must match the app's `FLASH_SIZE`.
const FLASH_SIZE: usize = 8 * 1024 * 1024;

#[entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());

    // The watchdog keeps running after the jump, so the app must feed it.
    let flash = WatchdogFlash::<FLASH_SIZE>::start(p.FLASH, p.WATCHDOG, Duration::from_secs(8));
    let flash = Mutex::new(RefCell::new(flash));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();
    let bl: BootLoader = BootLoader::prepare(config);

    // SAFETY: `load` jumps to the vector table at the start of ACTIVE. The app is linked to start
    // there (the shared layout), and `prepare` has already finished any swap or rollback, so ACTIVE
    // holds a whole image: the flashed one, or one the app staged after checking its signature.
    unsafe { bl.load(embassy_rp::flash::FLASH_BASE as u32 + active_offset) }
}

#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
    // SAFETY: ICSR is a core register, always mapped and readable.
    let irqn = unsafe { core::ptr::read_volatile(SCB_ICSR) } as u8 as i16 - 16;

    panic!("DefaultHandler #{:?}", irqn);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::asm::udf();
}
