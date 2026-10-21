//! The board's keys: P-256 keys that live in the TPM and never leave it.
//!
//! - The identity key signs. A host proves the board holds it with a challenge: it sends a random
//!   nonce, and the board signs `SHA-256(CHALLENGE_DOMAIN || nonce)` (ECDSA, `r || s`).
//!   `tools/challenge.py` does this.
//! - The decryption key does ECDH: it's what the update server will encrypt each update's key to.
//!   It can't sign, and the identity key can't do ECDH (key separation).

use sha2::{Digest, Sha256};
use tpm::{KeyKind, PublicKey, Signature};

use crate::{Tpm, TpmError};

/// Where the keys persist: the first owner handles after 0x81000001, which TCG reserves for the
/// storage root key.
const IDENTITY_HANDLE: u32 = 0x8100_0002;
const DECRYPT_HANDLE: u32 = 0x8100_0003;

/// Prefix of every signed challenge. It separates challenge signatures from anything else this key
/// signs, so `sign` can't be used to forge those. Must match `DOMAIN` in `tools/challenge.py`.
const CHALLENGE_DOMAIN: &[u8] = b"kb2040-fw-update/challenge/v1";
/// Challenge nonces are exactly this long.
pub const NONCE_LEN: usize = 32;

/// The identity key ([`KeyKind::Signing`]) or the decryption key, created and made persistent on
/// first use. `true` if this call created it.
pub async fn key(tpm: &mut Tpm, kind: KeyKind) -> Result<(PublicKey, bool), TpmError> {
    let handle = match kind {
        KeyKind::Signing => IDENTITY_HANDLE,
        KeyKind::Decrypt => DECRYPT_HANDLE,
    };
    tpm.startup().await?;
    if let Some(key) = tpm.read_key(handle, kind).await? {
        return Ok((key, false));
    }
    let (transient, key) = tpm.create_key(kind).await?;
    let persisted = tpm.persist(transient, handle).await;
    // Flush the transient copy even if persisting failed: the TPM has only 3 object slots.
    let flushed = tpm.flush(transient).await;
    persisted?;
    flushed?;
    Ok((key, true))
}

/// Signs a challenge with the identity key, creating the key on first use.
pub async fn sign_challenge(tpm: &mut Tpm, nonce: &[u8; NONCE_LEN]) -> Result<Signature, TpmError> {
    key(tpm, KeyKind::Signing).await?;
    let digest = Sha256::new()
        .chain_update(CHALLENGE_DOMAIN)
        .chain_update(nonce)
        .finalize();
    tpm.sign(IDENTITY_HANDLE, &digest.into()).await
}
