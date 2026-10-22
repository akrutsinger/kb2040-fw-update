# kb2040-fw-update

Example for how to do encrypted firmware updates using the Adafruit KB2040 (RP2040, 8 MB flash).

**The book, <https://akrutsinger.github.io/kb2040-fw-update/>, explains how it all works**, one
step at a time, with the code. This README has just the commands to build the firmware, install
it, and update it through the update server. Its sources are in `book/`.

## Layout

- `firmware/`: cargo workspace for the board: `app/` and its `bootloader/`
- `crates/`: libraries the app uses, built and tested on the host: `tpm/` (TPM driver) and `hpke/`
- `tools/`: Python scripts that sign, upload and release images, and talk to the board
- `keys/update.pub`: public half of the signing key, built into the app
- `worker/`: the update server, a Cloudflare Worker
- `book/`: the book ([mdBook](https://rust-lang.github.io/mdBook/))

## Prerequisites

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
cargo install cargo-binutils && rustup component add llvm-tools   # cargo objcopy, for updates
```

[uv](https://docs.astral.sh/uv/) runs the scripts in `tools/` and fetches their dependencies
(pyserial, cryptography). [Node.js](https://nodejs.org/) with npm runs the update server locally
(`worker/`).

### Signing key

The app only accepts updates signed with the project's Ed25519 key. Make it once, from the repo
root:

```sh
ssh-keygen -t ed25519 -N "" -C kb2040-fw-update -f keys/update
```

Commit `keys/update.pub`, which the app builds in. Keep `keys/update` safe and out of git (it's in
`.gitignore`): anyone who has it can sign images the board will install, and if it's lost, boards
can only be updated by BOOTSEL. See [Setup](https://akrutsinger.github.io/kb2040-fw-update/setup.html).

## Build

From `firmware/`:

```sh
cargo build --release              # all crates
cargo build --release -p app       # just one
```

Run cargo from inside `firmware/`, so it picks up the target from `firmware/.cargo/config.toml`.
Linting and the host tests are in
[Build, lint and test](https://akrutsinger.github.io/kb2040-fw-update/building.html).

## Flash (UF2)

1. Put the board in BOOTSEL: hold **BOOT** and tap **RESET** (or plug in while holding BOOT).
   It mounts as `RPI-RP2`, and `lsusb` shows `2e8a:0003 Raspberry Pi RP2 Boot`.
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

The app doesn't boot without the bootloader. Until an app is flashed, the bootloader faults and
resets in a loop. That's harmless; BOOTSEL still works.

## TPM

The board's keys live in an Infineon SLB 9670 TPM 2.0, on a
[52Pi TPM9670](https://wiki.52pi.com/index.php/EP-0149) board:

| TPM9670 pin | Signal | KB2040                            |
| ----------- | ------ | --------------------------------- |
| 1           | 3.3 V  | 3V                                |
| 6           | GND    | GND                               |
| 19          | MOSI   | MO (GP19)                         |
| 20          | GND    | GND, on a wire twisted around SCK |
| 21          | MISO   | MI (GP20)                         |
| 23          | SCLK   | SCK (GP18)                        |
| 26          | CS#    | D10 (GP10)                        |

`tpm` on the console should end with `TPM 2.0, started`. If it doesn't, see
[Wiring](https://akrutsinger.github.io/kb2040-fw-update/tpm.html). If it says
`ERR TPM runs TPM 1.2 firmware`, the chip needs a firmware change first, see
[TPM 1.2 firmware](https://akrutsinger.github.io/kb2040-fw-update/tpm-firmware.html).

## Update server

`worker/` is the update server, a [Cloudflare Worker](https://developers.cloudflare.com/workers/).
`wrangler dev` runs it locally. It checks that an update request comes from a registered board,
and offers that board a newer release, encrypted so that only the board's TPM can read it. The
board has no network, so `tools/bridge.py` carries its messages over USB serial. How it works is
in [The update server](https://akrutsinger.github.io/kb2040-fw-update/update-server.html) and
[How releases are protected](https://akrutsinger.github.io/kb2040-fw-update/releases.html).

### Encrypted updates, step by step

Steps 1 to 4 are once only; steps 5 to 9 are for each new release. Run the commands from the repo
root unless a step says otherwise. Close tio before any step that talks to the board (3, 4, 9):
two programs on one port split the data between them.

**Once:**

1. **Install the server's dependencies and make its secrets.** From `worker/`:

   ```sh
   npm install
   echo "NONCE_KEY=$(openssl rand -hex 32)" > .dev.vars     # local secrets, gitignored
   echo "RELEASE_KEK=$(openssl rand -hex 32)" >> .dev.vars
   ```

2. **Start the server.** From `worker/`, in a terminal of its own, and leave it running for the
   other steps. It serves on `http://localhost:8787`, and keeps its data in `worker/.wrangler/`
   across restarts.

   ```sh
   npm run dev
   ```

3. **Put firmware on the board that knows `update encrypted`,** the old way: BOOTSEL and
   `cargo run --release -p app` from `firmware/` (see [Flash (UF2)](#flash-uf2)), or a signed
   image with `upload.py` (see
   [Updates over USB](https://akrutsinger.github.io/kb2040-fw-update/usb-update.html)). After
   this, new firmware can come encrypted.

4. **Enroll the board** with the server. This reads its two public keys, creating them on first
   use:

   ```sh
   uv run tools/enroll.py /dev/ttyACM0
   ```

**For each release:**

5. **Bump `version`** in `firmware/app/Cargo.toml`. Boards are only offered versions newer than
   theirs. Changing `LED_COLOR` in `main.rs` too makes the update visible.
6. **Build the raw image.** From `firmware/`:

   ```sh
   cargo objcopy --release -p app -- -O binary target/app.bin
   ```

7. **Sign it** with the update key. This writes `firmware/target/app.signed`:

   ```sh
   uv run tools/sign.py firmware/target/app.bin
   ```

8. **Publish it** as an encrypted release on the server:

   ```sh
   uv run tools/release.py firmware/target/app.signed
   ```

9. **Install it on the board.** `bridge.py` asks the server for an update on the board's behalf,
   then downloads the encrypted image and installs it:

   ```sh
   uv run tools/bridge.py /dev/ttyACM0
   ```

   ```
   device   71ee3c8ba4aa108e651b4c50137feda4
   running  0.1.0
   latest   0.2.0
   authorized
   update   0.2.0, 86080 encrypted bytes
   100%  86016/86016 bytes
   verified in 2.7 s
   done, the board is rebooting into 0.2.0
   ```

   The board reboots into the new image on trial, and confirms it after a 5 s self-test. Then
   `version` in tio shows the new version and `status` says `update installed`, and running
   `bridge.py` again prints `up to date`.

## Book

To read the book locally (`cargo install mdbook`), from the repo root:

```sh
mdbook serve book --open
```
