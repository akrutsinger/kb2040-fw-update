# Updates over USB

Once the bootloader and an app are on the board, new app images can go over the USB serial
console instead of BOOTSEL. This sends the image unencrypted. For encrypted releases through the
update server, see [Encrypted updates, step by step](encrypted-updates.md).

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

## Signing

`sign.py` signs the SHA-512 digest of the padded image, since that's what embassy-boot's
`verify_and_mark_updated` checks:

```python
{{#include ../../tools/sign.py:sign}}
```

## Checking the signature

The board writes the image to DFU, then reads it back and checks the signature against
`keys/update.pub`. Only then does it mark the image and reboot. An unsigned, tampered or wrongly
signed image gets `ERR signature check failed`, and the board keeps running the current one.
`upload.py` refuses files that aren't `.signed` output before talking to the board.

The receiving end, in `firmware/app/src/update.rs`. Each block is written to DFU as it arrives,
and the signature comes last (the `cipher` is for [encrypted updates](releases.md)):

```rust,ignore
{{#include ../../firmware/app/src/update.rs:stage}}
```

The whole protocol is described at the top of `firmware/app/src/update.rs`.

Checking the signature takes about 2.6 s for a 36 KB image and 2.7 s for a 78 KB one: almost all
of it is the fixed cost of the Ed25519 math, and hashing adds only about 0.1 s per 40 KB. The board
can't feed the watchdog meanwhile (8 s timeout), but even a full 768 KB image should take only
about 4.5 s. If it ever ran over, the board would reset mid-check. That's safe, since the image
isn't marked and the old one keeps running, but `upload.py` only reports `finish: no reply`, which
isn't clear and can be confusing.

## Trial boot

On reboot the bootloader swaps it into ACTIVE and boots it on trial: after a 5 s self-test the app
confirms itself (`mark_booted`). If it panics, hangs (watchdog, 8 s) or resets before that, the
bootloader rolls back to the previous image. `status` on the console shows which happened:

```rust,ignore
{{#include ../../firmware/app/src/main.rs:trial-boot}}
```
