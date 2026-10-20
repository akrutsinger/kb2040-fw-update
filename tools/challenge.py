#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyserial", "cryptography"]
# ///
"""Check that the board holds its identity key: send it a random challenge, verify the signature.

usage: uv run tools/challenge.py PORT

Reads the board's public key (`pubkey`), has it sign a fresh random nonce (`sign`), and verifies
the ECDSA P-256 signature over SHA-256(DOMAIN || nonce). firmware/app/src/identity.rs describes
the format.
"""

import secrets
import sys
import time

import serial
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature

# Must match CHALLENGE_DOMAIN in identity.rs.
DOMAIN = b"kb2040-fw-update/challenge/v1"
NONCE_LEN = 32
# Seconds to wait for a reply. The first `pubkey` creates the key, which takes a moment.
TIMEOUT = 10


def command(port, line):
    """Runs a console command. Returns the last line of its reply, or exits on ERR."""
    name = line.split()[0]
    port.write(line.encode() + b"\r")
    reply = port.read_until(b"> ")
    if not reply.endswith(b"> "):
        sys.exit(f"{name}: no reply; is another program (tio?) holding the port?")
    # Drop the echoed command line and the prompt.
    lines = reply[:-2].decode(errors="replace").splitlines()[1:]
    for text in lines:
        if text.startswith("ERR"):
            sys.exit(f"{name}: {text}")
    if not lines:
        sys.exit(f"{name}: empty reply")
    return lines[-1].strip()


def from_hex(name, text, length):
    try:
        data = bytes.fromhex(text)
    except ValueError:
        data = b""
    if len(data) != length:
        sys.exit(f"{name}: expected {length} bytes of hex, got {text!r}")
    return data


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    nonce = secrets.token_bytes(NONCE_LEN)

    with serial.Serial(sys.argv[1], timeout=TIMEOUT) as port:
        # Get a fresh prompt.
        port.write(b"\r")
        time.sleep(0.1)
        port.reset_input_buffer()
        point = from_hex("pubkey", command(port, "pubkey"), 65)
        signature = from_hex("sign", command(port, f"sign {nonce.hex()}"), 64)

    key = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), point)
    r = int.from_bytes(signature[:32], "big")
    s = int.from_bytes(signature[32:], "big")
    try:
        key.verify(
            encode_dss_signature(r, s), DOMAIN + nonce, ec.ECDSA(hashes.SHA256())
        )
    except InvalidSignature:
        sys.exit("signature INVALID: the board doesn't hold this key")
    print(f"key    {point.hex()}")
    print(f"nonce  {nonce.hex()}")
    print("signature valid: the board holds this key")


if __name__ == "__main__":
    main()
