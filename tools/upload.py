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

import sys

import serial

import board

# Seconds to wait for each reply from the board.
TIMEOUT = 5


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    port_name, path = sys.argv[1:]
    with open(path, "rb") as f:
        data = f.read()
    if len(data) <= board.SIGNATURE_SIZE or (len(data) - board.SIGNATURE_SIZE) % board.BLOCK_SIZE:
        sys.exit(f"{path} is not a signed image; sign it first: uv run tools/sign.py IMAGE.bin")

    with serial.Serial(port_name, timeout=TIMEOUT) as port:
        board.fresh_prompt(port)
        board.send_update(port, "update", data)
    print("done, the board is rebooting into the new image")


if __name__ == "__main__":
    main()
