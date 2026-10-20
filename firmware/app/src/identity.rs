//! The board's identity: a P-256 signing key that lives in the TPM and never leaves it.
//!
//! A host proves the board holds the key with a challenge: it sends a random nonce, and the board
//! signs `SHA-256(CHALLENGE_DOMAIN || nonce)` (ECDSA, `r || s`). `tools/challenge.py` does this.

use sha2::{Digest, Sha256};
use tpm::{PublicKey, Signature};

use crate::{Tpm, TpmError};

/// Where the key persists: the first owner handle after 0x81000001, which TCG reserves for the
/// storage root key.
const HANDLE: u32 = 0x8100_0002;

/// Prefix of every signed challenge. It separates challenge signatures from anything else this key
/// signs, so `sign` can't be used to forge those. Must match `DOMAIN` in `tools/challenge.py`.
const CHALLENGE_DOMAIN: &[u8] = b"kb2040-fw-update/challenge/v1";
/// Challenge nonces are exactly this long.
pub const NONCE_LEN: usize = 32;

/// The identity key, created and made persistent on first use. `true` if this call created it.
pub async fn public_key(tpm: &mut Tpm) -> Result<(PublicKey, bool), TpmError> {
    tpm.startup().await?;
    if let Some(key) = tpm.read_signing_key(HANDLE).await? {
        return Ok((key, false));
    }
    let (transient, key) = tpm.create_signing_key().await?;
    let persisted = tpm.persist(transient, HANDLE).await;
    // Flush the transient copy even if persisting failed: the TPM has only 3 object slots.
    let flushed = tpm.flush(transient).await;
    persisted?;
    flushed?;
    Ok((key, true))
}

/// Signs a challenge with the identity key, creating the key on first use.
pub async fn sign_challenge(tpm: &mut Tpm, nonce: &[u8; NONCE_LEN]) -> Result<Signature, TpmError> {
    public_key(tpm).await?;
    let digest = Sha256::new()
        .chain_update(CHALLENGE_DOMAIN)
        .chain_update(nonce)
        .finalize();
    tpm.sign(HANDLE, &digest.into()).await
}
