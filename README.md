# kb2040-fw-update

Example for how to do encrypted firmware updates using the Adafruit KB2040 (RP2040, 8 MB flash).

## Layout

- `firmware/`: cargo workspace
  - `memory-layout.x`: flash partitions shared by both crates (see [Memory layout](#memory-layout))
  - `app/`: the application, runs from the ACTIVE partition
  - `bootloader/`: [embassy-boot-rp](https://docs.embassy.dev/embassy-boot-rp/0.10.0/default/index.html) bootloader, owns boot2 and jumps to ACTIVE
  - `tpm/`: async driver for the TPM (see [TPM](#tpm)), a platform-agnostic library the app uses
- `tools/sign.py`: signs app images with the update key
- `tools/upload.py`: sends a signed app image to the board over USB serial (see [Update over USB](#update-over-usb))
- `keys/update.pub`: public half of the signing key (OpenSSH format), built into the app

## Prerequisites

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
cargo install cargo-binutils && rustup component add llvm-tools   # cargo objcopy, for updates
```

[uv](https://docs.astral.sh/uv/) runs the scripts in `tools/` and fetches their dependencies
(pyserial, cryptography).

### Signing key

The app only accepts updates signed with the project's Ed25519 key. It's a plain OpenSSH key, so
make it once with `ssh-keygen`, from the repo root:

```sh
ssh-keygen -t ed25519 -N "" -C kb2040-fw-update -f keys/update
```

This writes `keys/update` (private) and `keys/update.pub` (`ssh-ed25519 …` line). Commit
`update.pub`: the app's `build.rs` extracts the raw key from it and builds it in. Keep
`keys/update` safe and out of git (it's in `.gitignore`): anyone who has it can sign images the
board will install, and if it's lost, boards can only be updated by BOOTSEL. `sign.py` doesn't
support passphrases yet, so the key must be unencrypted (`-N ""`).

## Build

From `firmware/`:

```sh
cargo build --release              # all crates
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

The `tpm` driver has host tests (SPI framing and register logic, against mocks). They need the
host target, since `.cargo/config.toml` defaults to the board:

```sh
cargo test -p tpm --target x86_64-unknown-linux-gnu
```

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

3. From the repo root, sign it. This pads the image to whole 4 KB blocks and writes
   `firmware/target/app.signed` (image followed by its 64-byte signature):

   ```sh
   uv run tools/sign.py firmware/target/app.bin
   ```

4. Close tio (two programs on one port split the data between them), then:

   ```sh
   uv run tools/upload.py /dev/ttyACM0 firmware/target/app.signed
   ```

The board writes the image to DFU, then reads it back and checks the signature against
`keys/update.pub`. Only then does it mark the image and reboot. An unsigned, tampered or wrongly
signed image gets `ERR signature check failed`, and the board keeps running the current one.
`upload.py` refuses files that aren't `.signed` output before talking to the board.

Checking the signature takes about 2.6 s for a 36 KB image (about 14 KB/s), and the board can't
feed the watchdog meanwhile. With an 8 s timeout, images over about 100 KB get reset mid-check.
That's safe, since the image isn't marked and the old one keeps running, but `upload.py` only
reports `finish: no reply`, which isn't clear and can be confusing.

On reboot the bootloader swaps it into ACTIVE and
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

## TPM

A [52Pi TPM9670](https://wiki.52pi.com/index.php/EP-0149) board: an Infineon
[SLB 9670](https://www.infineon.com/assets/row/public/documents/30/49/infineon-slb-9670vq2.0-datasheet-en.pdf)
TPM 2.0 on a 26-pin Raspberry Pi header. The wiki only labels power and the SPI data pins; CS# is
on CE1 (pin 26), per the Raspberry Pi `tpm-slb9670` overlay.

| TPM9670 pin | Signal | KB2040     |
| ----------- | ------ | ---------- |
| 1           | 3.3 V  | 3V         |
| 6           | GND    | GND        |
| 19          | MOSI   | MO (GP19)  |
| 21          | MISO   | MI (GP20)  |
| 23          | SCLK   | SCK (GP18) |
| 26          | CS#    | D9 (GP9)   |

- RST# and PIRQ# stay unconnected. RST# has an internal pull-up, and the datasheet (section 5.5)
  warns that asserting it at the wrong moment can trip the TPM's dictionary-attack defenses. So
  resetting the KB2040 (watchdog, update) doesn't reset the TPM.
- SPI runs in mode 0 at 1 MHz, which is safe over jumper wires. The chip takes up to 43 MHz.
- CS# is on D9 because the D10 header pin on my KB2040 doesn't reach GP10: it reads 0 V while the
  firmware drives GP10 high. With CS# floating, the TPM answers with consistent garbage. If the
  TPM misbehaves, check that CS# idles at 3.3 V first.

`tpm` on the console reads the ID registers, then requests locality 0:

```
> tpm
Infineon TPM, did 001b rid 10
locality 0 active
```

Anything else is almost always wiring. embassy-rp leaves MISO floating, so when the TPM isn't
driving it (missing, unpowered, or CS# not reaching it), MISO picks up noise from the neighboring
wires. That shows up as `ERR TPM never ready`, an unexpected vendor, or a locality that's never
granted. Check CS#, MISO, and power.

The driver (`firmware/tpm/`) follows the register interface and SPI protocol of the TCG
"PC Client Platform TPM Profile (PTP) Specification". It takes an `SpiBus` plus a CS# pin rather
than an `SpiDevice`, because the TPM can stretch a transfer with wait states (see
`tpm/src/interface/spi.rs`).

## Board notes

- The KB2040 has no plain LED, only a WS2812 NeoPixel on **GPIO17**. The app drives it
  with embassy-rp's `PioWs2812` (PIO0 + DMA_CH0).
- SPI0 with DMA_CH1 and DMA_CH2 drives the TPM (see [TPM](#tpm)).
- I don't think the KB2040 supports a debug probe, so there is no defmt/RTT logging and panics use
  `panic-halt`. USB logging (`embassy-usb-logger`) may be the way to get output later.
