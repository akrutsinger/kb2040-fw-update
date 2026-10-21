"""The board's USB serial console, shared by the scripts in tools/. Not a script itself.

Scripts import it as `board` (Python puts the script's own directory on the import path) and list
pyserial in their own dependencies.
"""

import hashlib
import struct
import sys
import time

# One flash erase sector, must match BLOCK_SIZE in update.rs.
BLOCK_SIZE = 4096
SIGNATURE_SIZE = 64
# Seconds to wait for the final reply, while the board hashes and verifies the image.
VERIFY_TIMEOUT = 30


def fresh_prompt(port):
    """Ends any half-typed line and drops whatever the board has sent so far."""
    port.write(b"\r")
    time.sleep(0.1)
    port.reset_input_buffer()


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


def device_id(identity_key):
    """The update server's name for a board: the first 16 bytes of SHA-256(identity key), in hex.
    `identity_key` is the SEC1 point `pubkey` prints, as bytes."""
    return hashlib.sha256(identity_key).digest()[:16].hex()


def send_update(port, command, data, enc=b"", ticket=b""):
    """Runs the update protocol in firmware/app/src/update.rs: `command` (`update` or
    `update encrypted`), the ticket and its `enc` if there is one, then `data` (image and
    signature) in blocks. Exits on any ERR. On return, the board is rebooting into the new
    image."""
    image, signature = data[:-SIGNATURE_SIZE], data[-SIGNATURE_SIZE:]
    port.write(command.encode() + b"\r")
    # Skip the echoed command until the board is ready.
    while True:
        line = port.readline()
        if not line:
            sys.exit("no reply from the board; is another program (tio?) holding the port?")
        if line.strip() == b"READY":
            break
        if b"ERR" in line:
            _fail(command, line[line.index(b"ERR") :])

    if ticket:
        port.write(bytes([len(ticket)]))
        _expect_ack(port, "ticket length")
        port.write(enc + ticket)
        _expect_ack(port, "ticket")
    port.write(struct.pack("<I", len(image)))
    _expect_ack(port, "length")
    for offset in range(0, len(image), BLOCK_SIZE):
        port.write(image[offset : offset + BLOCK_SIZE])
        _expect_ack(port, f"block at {offset:#x}")
        done = offset + BLOCK_SIZE
        print(f"\r{done * 100 // len(image):3}%  {done}/{len(image)} bytes", end="", flush=True)
    print()

    port.write(signature)
    start = time.monotonic()
    port.timeout = VERIFY_TIMEOUT
    line = port.readline()
    print(f"verified in {time.monotonic() - start:.1f} s")
    if line.strip() != b"OK":
        _fail("finish", line)


def _fail(what, reply):
    text = reply.decode(errors="replace").strip()
    sys.exit(f"{what}: {text or 'no reply'}")


def _expect_ack(port, what):
    b = port.read(1)
    if b != b"A":
        # Not an ack: the board is sending an "ERR ..." line.
        _fail(what, b + port.readline())
