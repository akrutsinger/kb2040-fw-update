# Setup

## Prerequisites

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
cargo install cargo-binutils && rustup component add llvm-tools   # cargo objcopy, for updates
```

[uv](https://docs.astral.sh/uv/) runs the scripts in `tools/` and fetches their dependencies
(pyserial, cryptography). [Node.js](https://nodejs.org/) with npm runs the update server locally
(`worker/`).

## Signing key

The app only accepts updates signed with the project's Ed25519 key. It's a plain OpenSSH key, so
make it once with `ssh-keygen`, from the repo root:

```sh
ssh-keygen -t ed25519 -N "" -C kb2040-fw-update -f keys/update
```

This writes `keys/update` (private) and `keys/update.pub` (`ssh-ed25519 …` line). Commit
`update.pub`: the app's `build.rs` extracts the raw key from it and builds it in. Keep
`keys/update` safe and out of git (it's in `.gitignore`): anyone who has it can sign images the
board will install, and if it's lost, boards can only be updated by BOOTSEL. `sign.py` doesn't
support passphrases yet, so the key must be unencrypted (`-N ""`).
