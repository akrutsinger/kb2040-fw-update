# kb2040-fw-update

Example for how to do encrypted firmware updates using the Adafruit KB2040 (RP2040, 8 MB flash).

## Layout

- `firmware/`: cargo workspace
  - `app/`: the application (currently a NeoPixel blinky)
  - `bootloader/`: placeholder for the embassy-boot-rp bootloader (not built by default)

## Prerequisites

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
```

## Build

From `firmware/`:

```sh
cargo build --release -p app
```

The target (`thumbv6m-none-eabi`) is set in `firmware/.cargo/config.toml`. The workspace
`default-members` is `app`, so a bare `cargo build` builds only the app.

## Flash (UF2)

1. Put the board in BOOTSEL: hold **BOOT** and tap **RESET** (or plug in while holding BOOT).
   It mounts as `RPI-RP2` (here `/run/media/austyn/RPI-RP2`), and `lsusb` shows
   `2e8a:0003 Raspberry Pi RP2 Boot`.
2. From `firmware/`:

   ```sh
   cargo run --release -p app
   ```

   The runner (`elf2uf2-rs -d`) converts the ELF to UF2 and copies it to the drive. The
   board reboots into the app on its own and the drive unmounts.

## Board notes

- The KB2040 has no plain LED, only a WS2812 NeoPixel on **GPIO17**. The app drives it
  with embassy-rp's `PioWs2812` (PIO0 + DMA_CH0).
- I don't think the KB2040 supports a debug probe, so there is no defmt/RTT logging and panics use
  `panic-halt`. USB logging (`embassy-usb-logger`) may be the way to get output later.
- `firmware/app/memory.x` uses the whole 8 MB of flash after boot2. This will change once
  the bootloader takes its share.
