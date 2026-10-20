//! The board's identity: a P-256 signing key that lives in the TPM and never leaves it.

use tpm::PublicKey;

use crate::{Tpm, TpmError};

/// Where the key persists: the first owner handle after 0x81000001, which TCG reserves for the
/// storage root key.
const HANDLE: u32 = 0x8100_0002;

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
