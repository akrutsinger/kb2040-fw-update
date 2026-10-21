#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyserial"]
# ///
"""Register a board with the update server: store its identity and decryption keys.

usage: uv run tools/enroll.py PORT

Reads both public keys from the board (`pubkey` and `pubkey decrypt`, which create them on first
use) and stores them as `device:<id>` in the KV that `wrangler dev` uses (worker/.wrangler).
The server only accepts update requests from boards registered here.

Registering trusts whatever answers on PORT, so do it with the board in front of you.
"""

import datetime
import json
import subprocess
import sys
from pathlib import Path

import board
import serial

WORKER = Path(__file__).resolve().parent.parent / "worker"
# Seconds to wait for a reply. The first `pubkey` creates the key, which takes a moment.
TIMEOUT = 10


def public_key(port, command):
    text = board.command(port, command)
    try:
        key = bytes.fromhex(text)
    except ValueError:
        key = b""
    if len(key) != 65 or key[0] != 0x04:
        sys.exit(f"{command}: expected a 65-byte SEC1 point in hex, got {text!r}")
    return key


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with serial.Serial(sys.argv[1], timeout=TIMEOUT) as port:
        board.fresh_prompt(port)
        identity = public_key(port, "pubkey")
        decrypt = public_key(port, "pubkey decrypt")

    device = board.device_id(identity)
    record = {
        "identity": identity.hex(),
        "decrypt": decrypt.hex(),
        "enrolled": datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds"),
    }
    command = ["npx", "wrangler", "kv", "key", "put", "--binding", "KV", "--local"]
    result = subprocess.run(
        command + [f"device:{device}", json.dumps(record)], cwd=WORKER
    )
    if result.returncode != 0:
        sys.exit("wrangler failed; did you run `npm install` in worker/?")
    print(f"enrolled device {device}")


if __name__ == "__main__":
    main()
