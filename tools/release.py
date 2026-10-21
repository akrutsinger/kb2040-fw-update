#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography"]
# ///
"""Publish a signed app image as an encrypted release on the update server.

usage: uv run tools/release.py IMAGE.signed

Make IMAGE.signed with `uv run tools/sign.py IMAGE.bin`. The release's version is `version` in
firmware/app/Cargo.toml, so bump it before building.

The whole .signed file (image and signature) is encrypted once, with a fresh random AES-128 key in
CTR mode. That content key is stored wrapped with RELEASE_KEK (AES-256-GCM) from worker/.dev.vars.
For each board that asks, the server unwraps it and seals it to that board's decryption key.

Writes to the local storage that `wrangler dev` uses (worker/.wrangler):
- R2 kb2040-fw-update-releases/<version>.bin: the encrypted image
- KV release:<version>: its size, IV and wrapped key
- KV release:latest: <version>, last, so boards are only offered a complete release
"""

import json
import os
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

ROOT = Path(__file__).resolve().parent.parent
WORKER = ROOT / "worker"
CARGO_TOML = ROOT / "firmware" / "app" / "Cargo.toml"
# Must match the bucket in worker/wrangler.toml.
BUCKET = "kb2040-fw-update-releases"
# Must match RELEASE_KEY_AAD in worker/src/index.ts.
RELEASE_KEY_AAD = b"kb2040-fw-update/release-key/v1"
# As in upload.py: whole 4 KB blocks, then the 64-byte signature.
BLOCK_SIZE = 4096
SIGNATURE_SIZE = 64


def release_kek():
    """RELEASE_KEK from worker/.dev.vars, the same key the local server uses."""
    path = WORKER / ".dev.vars"
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        sys.exit(f"{path} not found; see the README's Update server section")
    for line in lines:
        name, _, value = line.partition("=")
        if name.strip() == "RELEASE_KEK":
            try:
                kek = bytes.fromhex(value.strip())
            except ValueError:
                kek = b""
            if len(kek) != 32:
                sys.exit(f"RELEASE_KEK in {path} must be 32 bytes in hex")
            return kek
    sys.exit(f"no RELEASE_KEK in {path}; see the README's Update server section")


def wrangler(*args):
    result = subprocess.run(["npx", "wrangler", *args, "--local"], cwd=WORKER)
    if result.returncode != 0:
        sys.exit("wrangler failed; did you run `npm install` in worker/?")


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    path = sys.argv[1]
    data = Path(path).read_bytes()
    if len(data) <= SIGNATURE_SIZE or (len(data) - SIGNATURE_SIZE) % BLOCK_SIZE:
        sys.exit(f"{path} is not a signed image; sign it first: uv run tools/sign.py IMAGE.bin")
    version = tomllib.loads(CARGO_TOML.read_text())["package"]["version"]
    # The app has its version string built in (for `version`). Catches an image built before the
    # bump.
    if version.encode() not in data:
        sys.exit(f"{path} doesn't contain version {version}; rebuild it after bumping the version")
    kek = release_kek()

    content_key = os.urandom(16)
    iv = os.urandom(16)
    encryptor = Cipher(algorithms.AES(content_key), modes.CTR(iv)).encryptor()
    encrypted = encryptor.update(data) + encryptor.finalize()
    nonce = os.urandom(12)
    wrapped = nonce + AESGCM(kek).encrypt(nonce, content_key, RELEASE_KEY_AAD + version.encode())
    record = {"size": len(encrypted), "iv": iv.hex(), "key": wrapped.hex()}

    with tempfile.TemporaryDirectory() as tmp:
        image = Path(tmp) / f"{version}.bin"
        image.write_bytes(encrypted)
        wrangler("r2", "object", "put", f"{BUCKET}/{version}.bin", "--file", str(image))
    wrangler("kv", "key", "put", "--binding", "KV", f"release:{version}", json.dumps(record))
    wrangler("kv", "key", "put", "--binding", "KV", "release:latest", version)
    print(f"released {version} ({len(encrypted)} bytes, encrypted)")


if __name__ == "__main__":
    main()
