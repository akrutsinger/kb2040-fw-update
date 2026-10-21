#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyserial"]
# ///
"""Send a signed app image to the KB2040 over its USB serial console.

usage: uv run tools/upload.py PORT IMAGE.signed

Make IMAGE.signed with `uv run tools/sign.py IMAGE.bin`.

The protocol is described at the top of firmware/app/src/update.rs.
"""

import struct
import sys
import time

import serial

import board

# One flash erase sector, must match BLOCK_SIZE in update.rs.
BLOCK_SIZE = 4096
# Seconds to wait for each reply from the device.
TIMEOUT = 5
# Seconds to wait for the final reply, while the device hashes and verifies the image.
VERIFY_TIMEOUT = 30
SIGNATURE_SIZE = 64


def fail(what, reply):
    text = reply.decode(errors="replace").strip()
    sys.exit(f"{what}: {text or 'no reply'}")


def expect_ack(port, what):
    b = port.read(1)
    if b != b"A":
        # Not an ack: the device is sending an "ERR ..." line.
        fail(what, b + port.readline())


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    port_name, path = sys.argv[1:]
    with open(path, "rb") as f:
        data = f.read()
    if len(data) <= SIGNATURE_SIZE or (len(data) - SIGNATURE_SIZE) % BLOCK_SIZE:
        sys.exit(
            f"{path} is not a signed image; sign it first: uv run tools/sign.py IMAGE.bin"
        )
    image, signature = data[:-SIGNATURE_SIZE], data[-SIGNATURE_SIZE:]

    with serial.Serial(port_name, timeout=TIMEOUT) as port:
        board.fresh_prompt(port)
        port.write(b"update\r")

        # Skip the echoed command until the device is ready.
        while True:
            line = port.readline()
            if not line:
                sys.exit(
                    "no reply from the device; is another program (tio?) holding the port?"
                )
            if line.strip() == b"READY":
                break
            if b"ERR" in line:
                fail("update", line[line.index(b"ERR") :])

        port.write(struct.pack("<I", len(image)))
        expect_ack(port, "length")

        for offset in range(0, len(image), BLOCK_SIZE):
            port.write(image[offset : offset + BLOCK_SIZE])
            expect_ack(port, f"block at {offset:#x}")
            done = offset + BLOCK_SIZE
            print(
                f"\r{done * 100 // len(image):3}%  {done}/{len(image)} bytes",
                end="",
                flush=True,
            )
        print()

        port.write(signature)
        start = time.monotonic()
        port.timeout = VERIFY_TIMEOUT
        line = port.readline()
        print(f"verified in {time.monotonic() - start:.1f} s")
        if line.strip() != b"OK":
            fail("finish", line)
    print("done, the board is rebooting into the new image")


if __name__ == "__main__":
    main()
