#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyserial"]
# ///
"""Ask the update server whether the board may update. The board has no network, so this script
carries the messages between it (USB serial) and the server (HTTP).

usage: uv run tools/bridge.py PORT [SERVER]

SERVER defaults to http://localhost:8787, where `npm run dev` in worker/ serves.

1. Gets a nonce from the server (POST /v1/challenge).
2. Has the board sign an update request for it (`request <nonce>`).
3. Sends the request to the server (POST /v1/update) and prints the answer.
4. If the server offers a newer release, downloads its encrypted image (GET /v1/firmware/<version>)
   and installs it with `update encrypted`, which gives the board the ticket first. The board opens
   the ticket with its TPM, decrypts the image as it arrives, and checks the signature before
   switching to it.

The bridge can't forge or alter a request: only the board's TPM can sign one, and the server
checks it against the key it registered for the board. It can't read the image either: the
ticket only opens on that board.
"""

import json
import sys
import urllib.error
import urllib.request

import board
import serial

DEFAULT_SERVER = "http://localhost:8787"
# Seconds to wait for a reply from the board. The first `pubkey` creates the key.
TIMEOUT = 10


def post(server, path, body):
    """POSTs `body` as JSON and returns the JSON reply, or exits with the server's error."""
    request = urllib.request.Request(
        server + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            return json.load(response)
    except urllib.error.HTTPError as e:
        try:
            message = json.load(e).get("error", e.reason)
        except ValueError:
            message = e.reason
        sys.exit(f"{path}: {e.code} {message}")
    except urllib.error.URLError as e:
        sys.exit(
            f"{path}: can't reach {server} ({e.reason}); is `npm run dev` running in worker/?"
        )


def download(server, offer):
    """The offered release's encrypted image, checked against the size the server announced."""
    try:
        with urllib.request.urlopen(server + offer["url"], timeout=TIMEOUT) as response:
            image = response.read()
    except urllib.error.URLError as e:
        sys.exit(f"{offer['url']}: download failed ({e})")
    if len(image) != offer["size"]:
        sys.exit(f"{offer['url']}: got {len(image)} bytes, expected {offer['size']}")
    return image


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    server = sys.argv[2].rstrip("/") if len(sys.argv) == 3 else DEFAULT_SERVER

    nonce = post(server, "/v1/challenge", {})["nonce"]
    with serial.Serial(sys.argv[1], timeout=TIMEOUT) as port:
        board.fresh_prompt(port)
        identity = bytes.fromhex(board.command(port, "pubkey"))
        reply = board.command(port, f"request {nonce}").split()
        if len(reply) != 2:
            sys.exit(f"request: expected '<version> <signature>', got {' '.join(reply)!r}")
        version, signature = reply

        answer = post(
            server,
            "/v1/update",
            {
                "device": board.device_id(identity),
                "version": version,
                "nonce": nonce,
                "signature": signature,
            },
        )
        print(f"device   {answer['device']}")
        print(f"running  {answer['running']}")
        print(f"latest   {answer['latest'] or 'no release yet'}")
        print("authorized")
        offer = answer["update"]
        if offer is None:
            print("up to date")
            return
        image = download(server, offer)
        ticket = bytes.fromhex(offer["ticket"])
        if len(ticket) > 255:
            sys.exit(f"ticket is {len(ticket)} bytes, the board takes at most 255")
        print(f"update   {offer['version']}, {len(image)} encrypted bytes")
        # The board must not have run another `request` since; it checks the ticket's nonce.
        board.send_update(
            port, "update encrypted", image, enc=bytes.fromhex(offer["enc"]), ticket=ticket
        )
    print(f"done, the board is rebooting into {offer['version']}")


if __name__ == "__main__":
    main()
