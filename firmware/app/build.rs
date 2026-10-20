//! Puts `memory.x` and the shared layout on the linker search path and adds the linker scripts.
//! Also turns the update signing key into the raw bytes `update.rs` includes.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    fs::copy("../memory-layout.x", out.join("memory-layout.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=../memory-layout.x");

    // No -Tlink-rp.x: boot2 belongs to the bootloader (see memory.x).
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");

    let pubkey = "../../keys/update.pub";
    fs::write(out.join("update.pub"), ed25519_from_openssh(pubkey)).unwrap();
    println!("cargo:rerun-if-changed={pubkey}");
}

/// The 32-byte key from an OpenSSH public key line: `ssh-ed25519 <base64 blob> [comment]`.
fn ed25519_from_openssh(path: impl AsRef<Path>) -> [u8; 32] {
    let path = path.as_ref();
    let text = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; make it with `ssh-keygen -t ed25519 -N \"\" -f keys/update`",
            path.display()
        )
    });
    let mut fields = text.split_whitespace();
    let (Some("ssh-ed25519"), Some(blob)) = (fields.next(), fields.next()) else {
        panic!("{}: not an ssh-ed25519 public key", path.display());
    };
    let blob = STANDARD
        .decode(blob)
        .unwrap_or_else(|e| panic!("{}: bad base64: {e}", path.display()));
    // The blob is two length-prefixed strings: the key type, then the key.
    blob.strip_prefix(b"\0\0\0\x0bssh-ed25519\0\0\0\x20")
        .and_then(|key| key.try_into().ok())
        .unwrap_or_else(|| panic!("{}: malformed ssh-ed25519 key", path.display()))
}
