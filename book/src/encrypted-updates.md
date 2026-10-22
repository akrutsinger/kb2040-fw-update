# Encrypted updates, step by step

Steps 1 to 4 need only done once; steps 5 to 9 are for each new release. Run the commands from the
repo root unless a step says otherwise. Close tio (or minicom or whatever console program you're
using) before any step that talks to the board (3, 4, 9): two programs on one port split the data
between them.

**Once:**

1. **Install the server's dependencies and make its secrets.** From `worker/`:

   ```sh
   npm install
   echo "NONCE_KEY=$(openssl rand -hex 32)" > .dev.vars     # local secrets, gitignored
   echo "RELEASE_KEK=$(openssl rand -hex 32)" >> .dev.vars
   ```

2. **Start the server.** From `worker/`, in a terminal of its own, and leave it running for the
   other steps. It serves on `http://localhost:8787`, and keeps its data in `worker/.wrangler/`
   across restarts.

   ```sh
   npm run dev
   ```

3. **Put firmware on the board that knows `update encrypted`,** the old way: BOOTSEL and
   `cargo run --release -p app` from `firmware/` (see [First flash](first-flash.md)), or a signed
   image with `upload.py` (see [Updates over USB](usb-update.md)). After this, new firmware can
   come encrypted.

4. **Enroll the board** with the server. This reads its two public keys, creating them on first
   use:

   ```sh
   uv run tools/enroll.py /dev/ttyACM0
   ```

**For each release:**

5. **Bump `version`** in `firmware/app/Cargo.toml`. Boards are only offered versions newer than
   theirs. Make your changes to the `firmware/app` such as changing `LED_COLOR` in `main.rs` for this
   toy example, too makes the update visible.

6. **Build the raw image.** From `firmware/`:

   ```sh
   cargo objcopy --release -p app -- -O binary target/app.bin
   ```

7. **Sign it** with the update key. This writes `firmware/target/app.signed`. Run from the repo
   root:

   ```sh
   uv run tools/sign.py firmware/target/app.bin
   ```

8. **Publish it** as an encrypted release on the server:

   ```sh
   uv run tools/release.py firmware/target/app.signed
   ```

9. **Install it on the board.** `bridge.py` asks the server for an update on the board's behalf,
   then downloads the encrypted image and installs it:

   ```sh
   uv run tools/bridge.py /dev/ttyACM0
   ```

   ```
   device   71ee3c8ba4aa108e651b4c50137feda4
   running  0.1.0
   latest   0.2.0
   authorized
   update   0.2.0, 86080 encrypted bytes
   100%  86016/86016 bytes
   verified in 2.7 s
   done, the board is rebooting into 0.2.0
   ```

   The board reboots into the new image on trial, and confirms it after a 5s self-test. Then
   `version` in tio shows the new version and `status` says `update installed`, and running
   `bridge.py` again prints `up to date`.
