#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography"]
# ///
"""Sign an app image with the update key.

usage: uv run tools/sign.py IMAGE.bin

The key is keys/update, an unencrypted OpenSSH Ed25519 key. Make it with
`ssh-keygen -t ed25519 -N "" -f keys/update`.

Pads the image with 0xFF to whole blocks and writes IMAGE.signed next to it: padded image
followed by the 64-byte Ed25519 signature of sha512(padded image). That is what embassy-boot's
verify_and_mark_updated checks.
"""

import hashlib
import sys
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import load_ssh_private_key

# One flash erase sector, must match BLOCK_SIZE in update.rs.
BLOCK_SIZE = 4096
KEY = Path(__file__).resolve().parent.parent / "keys" / "update"


def load_key():
    keygen = f'ssh-keygen -t ed25519 -N "" -f {KEY}'
    try:
        key = load_ssh_private_key(KEY.read_bytes(), password=None)
    except FileNotFoundError:
        sys.exit(f"{KEY} not found; make it with: {keygen}")
    except TypeError:
        # What cryptography raises for a passphrase-protected key.
        sys.exit(f"{KEY} has a passphrase; sign.py only supports unencrypted keys")
    except ValueError as e:
        sys.exit(f"{KEY} is not an OpenSSH private key ({e}); make one with: {keygen}")
    if not isinstance(key, Ed25519PrivateKey):
        sys.exit(f"{KEY} is not an Ed25519 key; make one with: {keygen}")
    return key


def sign(path):
    key = load_key()
    image = Path(path).read_bytes()
    # The device takes whole blocks; pad like erased flash.
    image += b"\xff" * (-len(image) % BLOCK_SIZE)
    # embassy-boot signs the digest, not the image itself.
    signature = key.sign(hashlib.sha512(image).digest())
    out = Path(path).with_suffix(".signed")
    out.write_bytes(image + signature)
    print(f"wrote {out} ({len(image)} byte image + signature)")


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sign(sys.argv[1])


if __name__ == "__main__":
    main()
