# kb2040-fw-update

Example for how to do encrypted firmware updates using the Adafruit KB2040 (RP2040, 8 MB flash).

## Layout

- `firmware/`: cargo workspace
  - `memory-layout.x`: flash partitions shared by both crates (see [Memory layout](#memory-layout))
  - `app/`: the application, runs from the ACTIVE partition
  - `bootloader/`: [embassy-boot-rp](https://docs.embassy.dev/embassy-boot-rp/0.10.0/default/index.html) bootloader, owns boot2 and jumps to ACTIVE
- `tools/upload.py`: sends a new app image to the board over USB serial (see [Update over USB](#update-over-usb))

## Prerequisites

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
cargo install cargo-binutils && rustup component add llvm-tools   # cargo objcopy, for updates
```

[uv](https://docs.astral.sh/uv/) runs `tools/upload.py` and fetches its dependency (pyserial).

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

3. The app shows up as a USB serial port (`lsusb` shows `c0de:cafe kb2040-fw-update KB2040 console`).
   Open it with `tio /dev/ttyACM0` (or whatever device yours is). The app prints nothing until you
   type, so press **ENTER** to get the `> ` prompt, then try `help`.

   The port needs the `dialout` group (`sudo usermod -aG dialout $USER`, then log back in, or as a
   temporary fix, run `newgrp dialout` in the shell until then).

The app doesn't boot on its own: it has no boot2 and is linked at `0x10007000`, so the
bootloader must be on the board. Until an app is flashed, the bootloader jumps into erased
flash, faults and resets in a loop. That's harmless; BOOTSEL still works.

## Update over USB

Once the bootloader and an app are on the board, new app images can go over the USB serial
console instead of BOOTSEL.

1. Bump `version` in `firmware/app/Cargo.toml` (and `LED_COLOR` in `main.rs` to see the change).
2. From `firmware/`, build a raw image:

   ```sh
   cargo objcopy --release -p app -- -O binary target/app.bin
   ```

3. Close tio (two programs on one port split the data between them), then from the repo root:

   ```sh
   uv run tools/upload.py /dev/ttyACM0 firmware/target/app.bin
   ```

The board writes the image to DFU, marks it, and reboots. The bootloader swaps it into ACTIVE and
boots it on trial: after a 5 s self-test the app confirms itself (`mark_booted`). If it panics,
hangs (watchdog, 8 s) or resets before that, the bootloader rolls back to the previous image.
`status` on the console shows which happened. The protocol is described at the top of
`firmware/app/src/update.rs`.

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
