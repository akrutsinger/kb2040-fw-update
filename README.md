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
- `tools/challenge.py`: checks that the board holds its identity key (see [Identity key](#identity-key))
- `tools/board.py`: console helpers that `upload.py` and `challenge.py` share (not a script itself)
- `tools/tpmfactoryupd-openssl11.patch`: lets Infineon's TPM firmware tool build on a current Raspberry Pi OS (see [TPM 1.2 firmware](#tpm-12-firmware))
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

Two more tests run the TPM2 commands against [swtpm](https://github.com/stefanberger/swtpm), a
software TPM 2.0 (`dnf install swtpm`). They're skipped by default:

```sh
cargo test -p tpm --target x86_64-unknown-linux-gnu -- --include-ignored
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

Checking the signature takes about 2.6 s for a 36 KB image and 2.7 s for a 78 KB one: almost all
of it is the fixed cost of the Ed25519 math, and hashing adds only about 0.1 s per 40 KB. The board
can't feed the watchdog meanwhile (8 s timeout), but even a full 768 KB image should take only
about 4.5 s. If it ever ran over, the board would reset mid-check. That's safe, since the image
isn't marked and the old one keeps running, but `upload.py` only reports `finish: no reply`, which
isn't clear and can be confusing.

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

| TPM9670 pin | Signal | KB2040                            |
| ----------- | ------ | --------------------------------- |
| 1           | 3.3 V  | 3V                                |
| 6           | GND    | GND                               |
| 19          | MOSI   | MO (GP19)                         |
| 20          | GND    | GND, on a wire twisted around SCK |
| 21          | MISO   | MI (GP20)                         |
| 23          | SCLK   | SCK (GP18)                        |
| 26          | CS#    | D10 (GP10)                        |

- RST# and PIRQ# stay unconnected. RST# has an internal pull-up, and the datasheet (section 5.5)
  warns that asserting it at the wrong moment can trip the TPM's dictionary-attack defenses. So
  resetting the KB2040 (watchdog, update) doesn't reset the TPM.
- SPI runs in mode 0 at 1 MHz, which is safe over jumper wires. The chip takes up to 43 MHz.
- The second ground (pin 20) runs on a wire twisted around the SCK wire. I think the SCK picks
  up the edges of MISO (pin 21 is next to pin 23), and the TPM saw extra clock edges. It still
  answered, but with data that was wrong the same way every time. A slower SPI clock didn't help,
  since it's the edges that couple, not the clock rate. On a Raspberry Pi the board plugs straight
  onto the header and doesn't have this problem. Pin 25 isn't a ground on the TPM9670 (no continuity
  to pin 6), so it doesn't help.

`tpm` on the console reads the ID registers, requests locality 0, and starts the TPM:

```
> tpm
Infineon TPM, did 001b rid 10
locality 0 active
TPM 2.0, started
```

If the first two lines fail, it's almost always wiring. embassy-rp leaves MISO floating, so when the
TPM isn't driving it (missing, unpowered, or CS# not reaching it), MISO picks up noise from the
neighboring wires. That shows up as `ERR TPM never ready`, an unexpected vendor, or a locality
that's never granted. Check CS#, MISO, and power. If the wrong values repeat exactly from one `tpm`
to the next (mine read vendor `93a4` and ACCESS `0x82`), noise on SCK is the more likely cause:
check the ground wire twisted around SCK (see above), and keep the jumpers short and apart from
each other. If only the third line fails with `ERR TPM runs TPM 1.2 firmware`, see the next
section.

### TPM 1.2 firmware

The SLB 9670 is sold with either TPM 1.2 or TPM 2.0 firmware. It's the same chip with the same
device ID (`001b`), so wiring and `tpm`'s first two lines can't tell them apart. Everything here
needs TPM 2.0: TPM 1.2 has none of its commands, no elliptic curves (only RSA and SHA-1), and
Infineon's TPM 1.2 firmware before 6.43 generates weak RSA keys
([ROCA, CVE-2017-15361](https://nvd.nist.gov/vuln/detail/CVE-2017-15361)). My TPM9670 was sold as
TPM 2.0 but came with TPM 1.2 firmware 6.40.190.0.

How it shows up:

- `tpm`: `ERR TPM runs TPM 1.2 firmware, needs 2.0 (see README)` on the third line. `pubkey` and
  `sign` fail with the same message. A TPM 1.2 answers TPM 2.0 commands in its own format (tag
  `0x00C4`), which the driver recognizes.
- A hint in the registers: `INTF_CAPS` bits 30..28 read `010` (TIS 1.3, the TPM 1.2 interface)
  instead of `011`. The revision ID also differs between the firmwares (`10` on mine).

#### Switching it to TPM 2.0

Infineon's `TPMFactoryUpd` can change the firmware between TPM 1.2 and TPM 2.0. It runs on Linux
against `/dev/tpm0`, so the easiest place is a Raspberry Pi, which the TPM9670 plugs straight onto.
Infineon doesn't publish the 1.2-to-2.0 image itself, but Supermicro does, for its own SLB 9670
module. Its package goes from TPM 1.2 6.40.190.0 (and 6.41 to 6.43) to TPM 2.0 7.62.3126.0, which
has the ROCA fix and everything this project needs.

Before you start:

- **Don't interrupt the update** (power, Ctrl-C, reboot). An interrupted update must be resumed with
  the exact same image, and only a few resume attempts are allowed. Use a solid power supply.
- **The number of firmware updates is limited.** `-info` shows how many are left.
- Anything stored in the TPM is lost. Nothing here depends on the TPM 1.2 contents.

Steps:

1. **Raspberry Pi:** install Raspberry Pi OS. Add these lines to `/boot/firmware/config.txt`,
   power off, plug the TPM9670 onto header pins 1 to 26 (its pin 1 on the Pi's pin 1), and boot:

   ```
   dtparam=spi=on
   dtoverlay=tpm-slb9670
   ```

   `ls /dev/tpm0` should show the device, and `dmesg | grep -i tpm` a 1.2 TPM.

2. **Download** from <https://www.supermicro.com/wdl/driver/TPM/> and check the MD5s against
   Supermicro's `CheckSum.txt` there:

   | File                                                     | MD5                                |
   | -------------------------------------------------------- | ---------------------------------- |
   | `AOM-TPM-9670VH_7.62.3126.0.zip`                         | `2c9fc9111ea46200c11dbbb5579dad36` |
   | `TPM_FU_v1.01.2529.00_ToolsOnly_Linux_SourceCode.tar.gz` | `a0c2dae43b28f42134047ab8e6b0d3ac` |

3. **Build the tool.** The source is from 2017, so two fixes are needed for a current Raspberry Pi
   OS (gcc 14, OpenSSL 3): don't turn warnings into errors, and port `Crypt.c` to the OpenSSL 1.1
   API with `tools/tpmfactoryupd-openssl11.patch` from this repo (copy it to the Pi):

   ```sh
   cd ~
   sudo apt install build-essential libssl-dev
   mkdir tpmfu && tar xzf TPM_FU_v1.01.2529.00_ToolsOnly_Linux_SourceCode.tar.gz -C tpmfu
   patch tpmfu/Source/Common/Crypt/Linux/Crypt.c < tpmfactoryupd-openssl11.patch
   cd tpmfu/Source/TPMFactoryUpd
   sed -i 's/-Werror/-Wno-error/' makefile
   make
   ```

   The OpenSSL deprecation warnings that remain are harmless: OpenSSL 3 still has those functions.

4. **Check the TPM:** `sudo ./TPMFactoryUpd -info`. Mine said:

   ```
   TPM family                        :    1.2
   TPM firmware version              :    6.40.190.0
   TPM enabled                       :    No
   TPM activated                     :    No
   TPM owner set                     :    No
   TPM deferred physical presence    :    No (Settable)
   Remaining updates                 :    64
   ```

   Also make sure nothing else uses the TPM during the update: `sudo lsof /dev/tpm0` should print
   nothing, and `/sys/class/misc/hw_random/rng_current` shouldn't be `tpm-rng-0`.

5. **Update.** Unzip the Supermicro package. Which update type works depends on `-info`
   (the tool's manual, `Doc/TPMFactoryUpd_UserManual.pdf` in the tarball, section 5.4):

   - **Owner not set, deferred physical presence `Yes` or `No (Settable)`** (the state above):
     `tpm12-PP`, naming the image for the installed version:

     ```sh
     cd ~/AOM-TPM-9670VH_7.62.3126.0/Firmware
     sudo ~/tpmfu/Source/TPMFactoryUpd/TPMFactoryUpd -update tpm12-PP \
         -firmware TPM12_6.40.190.0_to_TPM20_7.62.3126.0.BIN -log update.log
     ```

   - **Enabled, activated, owner not set:** the package's config file also works. It picks the image
     itself and uses `tpm12-takeownership`, which needs exactly that state:
     `sudo ~/tpmfu/Source/TPMFactoryUpd -update config-file -config TPM20_latest.cfg`, run from the
     `Firmware` folder.

6. **Verify:** reboot the Pi, then `sudo ./TPMFactoryUpd -info` should say TPM family 2.0, firmware
   7.62.3126.0. Move the TPM9670 back to the KB2040, and `tpm` should end with `TPM 2.0, started`.

If `/dev/tpm0` doesn't appear after an interrupted update, the kernel driver likely gave up because
the TPM's self-test failed in its half-updated state. The tarball's `Kernel/` folder has kernel
patches that ignore that failure, so the update can be resumed.

### Identity key

`pubkey` prints the board's identity key, a P-256 ECDSA signing key that the TPM generated and
never lets out. It's printed as a SEC1 uncompressed point in hex (`04 || x || y`):

```
> pubkey
created identity key
04a1b2...
```

- The first `pubkey` creates the key (TPM2_CreatePrimary in the owner hierarchy) and makes it
  persistent at handle `0x81000002`. After that, `pubkey` just reads it back, across resets and
  power loss. Clearing the TPM deletes it; a new one would then be different.
- It's a signing key only, unrestricted so the TPM signs any digest the firmware gives it. Key
  agreement for encrypted updates uses its own key (see [Decryption key](#decryption-key)).
- Creating it needs the owner hierarchy's password to be empty, which it is on a fresh TPM. If the
  TPM was used elsewhere and has an owner password, `pubkey` fails with an authorization error
  such as `ERR TPM error [TPM_RC_BAD_AUTH, session[1]]`.

`sign <nonce>` proves the board holds the key. The nonce is 32 random bytes in hex (64 digits). The
board signs `SHA-256("kb2040-fw-update/challenge/v1" || nonce)` and prints the ECDSA signature,
`r || s` in hex. The fixed prefix keeps `sign` from being used to sign anything else with this
key. `tools/challenge.py` does the whole exchange with a fresh nonce and checks the signature
(close tio first):

```sh
uv run tools/challenge.py /dev/ttyACM0
```

It verifies against the key the board itself reports, so it proves the board holds that key, not
that it's the board you expect. Pinning known keys comes with the Worker (roadmap step 5).

### Decryption key

`pubkey decrypt` prints the board's second key, in the same format. It's made the same way, on the
first `pubkey decrypt`, and persists at handle `0x81000003`.

- It does key agreement (ECDH) only, and can't sign. The identity key in turn can't do ECDH. Each
  key does one job, so a mistake in how one is used can't weaken the other.
- Encrypted updates will use it. The update server encrypts each update's AES key to this key
  (HPKE). The board gets that key back with TPM2_ECDH_ZGen, which multiplies the server's one-time
  public key by the private key inside the TPM.
- There's deliberately no console command that runs ECDH_ZGen on its own: anyone at the USB port
  could then use the board to decrypt update keys. It only runs inside an encrypted update.

The driver (`firmware/tpm/`) follows the register interface and SPI protocol of the TCG
"PC Client Platform TPM Profile (PTP) Specification". TPM2 commands and responses are encoded and
decoded by [`tpm2-protocol`](https://crates.io/crates/tpm2-protocol) (`no_std`, no allocation). It
takes an `SpiBus` plus a CS# pin rather than an `SpiDevice`, because the TPM can stretch a transfer
with wait states (see `tpm/src/interface/spi.rs`).

## Board notes

- The KB2040 has no plain LED, only a WS2812 NeoPixel on **GPIO17**. The app drives it
  with embassy-rp's `PioWs2812` (PIO0 + DMA_CH0).
- SPI0 with DMA_CH1 and DMA_CH2 drives the TPM (see [TPM](#tpm)).
- I don't think the KB2040 supports a debug probe, so there is no defmt/RTT logging and panics use
  `panic-halt`. USB logging (`embassy-usb-logger`) may be the way to get output later.
