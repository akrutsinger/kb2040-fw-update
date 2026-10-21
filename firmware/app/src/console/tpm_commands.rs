//! The TPM commands: `tpm`, `pubkey [decrypt]`, `sign` and `request`.

use tpm::KeyKind;

use super::{Out, parse_hex};
use crate::{Tpm, identity};

/// TCG vendor ID of Infineon, who make the SLB 9670.
const INFINEON: u16 = 0x15D1;

/// Prints the TPM's IDs, takes locality 0, and starts the TPM, which also shows whether it runs
/// TPM 2.0 firmware. A wrong vendor usually means wiring: MISO floats when the TPM isn't driving
/// it, so the ID reads as noise.
pub(super) async fn info(out: &mut Out, tpm: &mut Tpm) {
    match tpm.id().await {
        Ok(id) if id.vid == INFINEON => reply!(
            out,
            "Infineon TPM, did {:04x} rid {:02x}\r\n",
            id.did,
            id.rid
        ),
        Ok(id) => reply!(
            out,
            "ERR unexpected vendor {:04x}, check wiring\r\n",
            id.vid
        ),
        Err(e) => reply!(out, "ERR {e}\r\n"),
    }
    match tpm.request_locality().await {
        Ok(()) => reply!(out, "locality 0 active\r\n"),
        Err(e) => {
            reply!(out, "ERR {e}\r\n");
            // Starting the TPM requests the locality again, so it would only repeat this error.
            return;
        }
    }
    match tpm.startup().await {
        Ok(()) => reply!(out, "TPM 2.0, started\r\n"),
        Err(e) => reply!(out, "ERR {e}\r\n"),
    }
}

/// Prints the identity key (`pubkey`) or the decryption key (`pubkey decrypt`) as a SEC1
/// uncompressed point in hex, creating it on first use.
pub(super) async fn pubkey(out: &mut Out, tpm: &mut Tpm, kind: KeyKind) {
    match identity::key(tpm, kind).await {
        Ok((key, created)) => {
            if created {
                let name = match kind {
                    KeyKind::Signing => "identity",
                    KeyKind::Decrypt => "decryption",
                };
                reply!(out, "created {name} key\r\n");
            }
            out.hex(&key.to_sec1());
            reply!(out, "\r\n");
        }
        Err(e) => reply!(out, "ERR {e}\r\n"),
    }
}

/// `sign <nonce>`: signs a challenge with the identity key and prints the signature, `r || s` in
/// hex. The nonce is 64 hex digits; see [`identity`] for what gets signed.
pub(super) async fn sign(out: &mut Out, tpm: &mut Tpm, nonce: &[u8]) {
    let Some(nonce) = parse_nonce(out, nonce) else {
        return;
    };
    match identity::sign_challenge(tpm, &nonce).await {
        Ok(signature) => {
            out.hex(&signature.r);
            out.hex(&signature.s);
            reply!(out, "\r\n");
        }
        Err(e) => reply!(out, "ERR {e}\r\n"),
    }
}

/// `request <nonce>`: an update request for the update server, which sends the nonce. Prints the
/// firmware version and the identity key's signature over the request (`r || s` in hex), as
/// `<version> <signature>`; see [`identity`] for what gets signed. Keeps the nonce in `last`, for
/// `update encrypted` to check its ticket against.
pub(super) async fn request(
    out: &mut Out,
    tpm: &mut Tpm,
    last: &mut Option<[u8; identity::NONCE_LEN]>,
    nonce: &[u8],
) {
    let Some(nonce) = parse_nonce(out, nonce) else {
        return;
    };
    match identity::sign_request(tpm, &nonce).await {
        Ok(signature) => {
            *last = Some(nonce);
            reply!(out, "{} ", identity::VERSION);
            out.hex(&signature.r);
            out.hex(&signature.s);
            reply!(out, "\r\n");
        }
        Err(e) => reply!(out, "ERR {e}\r\n"),
    }
}

/// The nonce argument of `sign` and `request`, or `None` after printing what's wrong with it.
fn parse_nonce(out: &mut Out, hex: &[u8]) -> Option<[u8; identity::NONCE_LEN]> {
    let nonce = parse_hex(hex.trim_ascii());
    if nonce.is_none() {
        reply!(out, "ERR nonce must be 64 hex digits\r\n");
    }
    nonce
}
