"""The board's USB serial console, shared by the scripts in tools/. Not a script itself.

Scripts import it as `board` (Python puts the script's own directory on the import path) and list
pyserial in their own dependencies.
"""

import sys
import time


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
