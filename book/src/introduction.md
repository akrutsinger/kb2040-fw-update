# Introduction

An example of how to do encrypted firmware updates on the Adafruit KB2040 (RP2040, 8 MB flash).

The book builds it up one piece at a time:

1. **Build and flash** the bootloader and the app the plain way, with BOOTSEL and a UF2 file.
2. **Sign** app images with an Ed25519 key, and send them over USB serial. The board checks the
   signature before it installs anything, boots the new image on trial, and rolls back if it
   doesn't confirm itself.
3. **Give the board an identity** with a TPM: keys that the TPM generates and never lets out.
4. **Run an update server** that only answers boards it knows, and hands each one a release
   encrypted so that only that board's TPM can open it.

Each chapter uses what the ones before it set up, so they're best read in order. The
[README](https://github.com/akrutsinger/kb2040-fw-update#readme) has just the commands, for
when you already know the way.

## Layout

- `firmware/`: cargo workspace
  - `memory-layout.x`: flash partitions shared by both crates (see [Memory layout](memory-layout.md))
  - `app/`: the application, runs from the ACTIVE partition
  - `bootloader/`: [embassy-boot-rp](https://docs.embassy.dev/embassy-boot-rp/0.10.0/default/index.html) bootloader, owns boot2 and jumps to ACTIVE
- `crates/`: cargo workspace for the platform-agnostic libraries the app uses, built and tested on the host
  - `tpm/`: async driver for the TPM (see [Wiring](tpm.md))
  - `hpke/`: opens HPKE messages (RFC 9180), for the app's update tickets (see [How releases are protected](releases.md))
- `tools/sign.py`: signs app images with the update key
- `tools/upload.py`: sends a signed app image to the board over USB serial (see [Updates over USB](usb-update.md))
- `tools/challenge.py`: checks that the board holds its identity key and signs update requests (see [Identity key](identity-key.md))
- `tools/enroll.py`: registers a board's keys with the update server (see [The update server](update-server.md))
- `tools/bridge.py`: carries a board's update request to the update server and back
- `tools/release.py`: publishes a signed image as an encrypted release (see [How releases are protected](releases.md))
- `tools/board.py`: console helpers the scripts share (not a script itself)
- `tools/tpmfactoryupd-openssl11.patch`: lets Infineon's TPM firmware tool build on a current Raspberry Pi OS (see [TPM 1.2 firmware](tpm-firmware.md))
- `keys/update.pub`: public half of the signing key (OpenSSH format), built into the app
- `worker/`: the update server, a Cloudflare Worker (see [The update server](update-server.md))
- `book/`: this book ([mdBook](https://rust-lang.github.io/mdBook/))
