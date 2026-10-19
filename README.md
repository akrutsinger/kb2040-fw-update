# kb2040-fw-update

Example for how to do encrypted firmware updates using the Adafruit KB2040 (RP2040, 8 MB flash).

## Layout

- `firmware/`: cargo workspace
  - `memory-layout.x`: flash partitions shared by both crates (see [Memory layout](#memory-layout))
  - `app/`: the application (currently a NeoPixel blinky), runs from the ACTIVE partition
  - `bootloader/`: embassy-boot-rp bootloader, owns boot2 and jumps to ACTIVE

## Prerequisites

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
```

## Build

From `firmware/`:

```sh
cargo build --release              # both crates
cargo build --release -p app       # just one
```

The target (`thumbv6m-none-eabi`) is set in `firmware/.cargo/config.toml`.

## Check / lint

From `firmware/`:

```sh
cargo check
cargo clippy
```

- Run them from inside `firmware/`. With `--manifest-path` from the repo root, cargo doesn't
  see `.cargo/config.toml` and builds for the host (`unwinding panics are not supported without std`).

## Flash (UF2)

1. Put the board in BOOTSEL: hold **BOOT** and tap **RESET** (or plug in while holding BOOT).
   It mounts as `RPI-RP2` (here `/run/media/austyn/RPI-RP2`), and `lsusb` shows
   `2e8a:0003 Raspberry Pi RP2 Boot`.
2. From `firmware/`:

   ```sh
   cargo run --release -p bootloader   # once, or when the bootloader changes
   cargo run --release -p app
   ```

   The runner (`elf2uf2-rs -d`) converts the ELF to UF2 and copies it to the drive. The
   board reboots on its own and the drive unmounts, so **each flash needs its own trip into
   BOOTSEL**. Each UF2 only writes its own partition, so flashing one leaves the other intact.

The app doesn't boot on its own: it has no boot2 and is linked at `0x10007000`, so the
bootloader must be on the board. Until an app is flashed, the bootloader jumps into erased
flash, faults and resets in a loop. That's harmless; BOOTSEL still works.

## Memory layout

Defined once in `firmware/memory-layout.x`. Each crate's `memory.x` includes it and points
`FLASH` at its own region (`REGION_ALIAS`).

| Region           | Start        | Size        | Notes                                              |
| ---------------- | ------------ | ----------- | -------------------------------------------------- |
| BOOT2            | `0x10000000` | 256 B       | second-stage boot, linked into the bootloader only |
| BOOTLOADER       | `0x10000100` | 24K - 256 B |                                                    |
| BOOTLOADER_STATE | `0x10006000` | 4K          | embassy-boot swap/state flags                      |
| ACTIVE           | `0x10007000` | 768K        | the running app                                    |
| DFU              | `0x100C7000` | 772K        | incoming update; ACTIVE + one 4K page for the swap |
| STORAGE          | `0x107F0000` | 64K         | reserved for data kept across boots (end of flash) |

Flash from `0x10188000` to `0x107F0000` (about 6.4 MB) is unused.

App size: use `size target/thumbv6m-none-eabi/release/app` (`text + data` is what gets flashed).
The ELF file itself is much bigger because of debug info.

## Board notes

- The KB2040 has no plain LED, only a WS2812 NeoPixel on **GPIO17**. The app drives it
  with embassy-rp's `PioWs2812` (PIO0 + DMA_CH0).
- I don't think the KB2040 supports a debug probe, so there is no defmt/RTT logging and panics use
  `panic-halt`. USB logging (`embassy-usb-logger`) may be the way to get output later.
