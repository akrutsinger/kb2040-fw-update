//! HPKE (RFC 9180) for the receiving side: single-shot open in base mode, for one cipher suite,
//! DHKEM(P-256, HKDF-SHA256), HKDF-SHA256 and AES-128-GCM.
//!
//! The KEM's Diffie-Hellman step is left to the caller, so this crate never sees a private key. On
//! the board the TPM does it (TPM2_ECDH_ZGen), and the tests use the `p256` crate.

#![no_std]

use aes_gcm::aead::AeadInOut;
use aes_gcm::{Aes128Gcm, KeyInit, Tag};
use hkdf::{Hkdf, HkdfExtract};
use sha2::Sha256;

/// Length of `enc`, the sender's encapsulated key: a SEC1 uncompressed P-256 point.
pub const ENC_LEN: usize = 65;
/// Length of the AEAD tag at the end of every ciphertext.
pub const TAG_LEN: usize = 16;

/// `"KEM" || kem_id`: DHKEM(P-256, HKDF-SHA256) is 0x0010.
const KEM_SUITE: &[u8] = b"KEM\x00\x10";
/// `"HPKE" || kem_id || kdf_id || aead_id`: HKDF-SHA256 is 0x0001, AES-128-GCM 0x0001.
const HPKE_SUITE: &[u8] = b"HPKE\x00\x10\x00\x01\x00\x01";
const MODE_BASE: u8 = 0x00;

/// Opening failed: the ciphertext is too short, or it doesn't authenticate under this key, `info`
/// and `aad`. The two aren't told apart, as usual for AEADs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenError;

/// Opens a single-shot HPKE message (the first one of its context, sequence number 0).
///
/// - `dh`: the x-coordinate of `skR * enc`, the KEM's Diffie-Hellman output.
/// - `enc`: the sender's encapsulated key, as it came with the message.
/// - `pk_r`: the recipient's public key, a SEC1 uncompressed point.
/// - `message`: ciphertext and tag. It's decrypted in place, and the plaintext (`message` minus the
///   tag) is returned.
pub fn open<'m>(
    dh: &[u8; 32],
    enc: &[u8; ENC_LEN],
    pk_r: &[u8; ENC_LEN],
    info: &[u8],
    aad: &[u8],
    message: &'m mut [u8],
) -> Result<&'m mut [u8], OpenError> {
    let split = message.len().checked_sub(TAG_LEN).ok_or(OpenError)?;
    let (plaintext, tag) = message.split_at_mut(split);
    let (key, nonce) = key_schedule(&shared_secret(dh, enc, pk_r), info);
    let tag = Tag::try_from(&*tag).map_err(|_| OpenError)?;
    Aes128Gcm::new(&key.into())
        .decrypt_inout_detached(&nonce.into(), aad, plaintext.into(), &tag)
        .map_err(|_| OpenError)?;
    Ok(plaintext)
}

/// DHKEM's `ExtractAndExpand(dh, enc || pkRm)` (RFC 9180, section 4.1).
fn shared_secret(dh: &[u8; 32], enc: &[u8; ENC_LEN], pk_r: &[u8; ENC_LEN]) -> [u8; 32] {
    let (_, eae_prk) = labeled_extract(KEM_SUITE, &[], b"eae_prk", dh);
    let mut shared_secret = [0; 32];
    labeled_expand(
        &eae_prk,
        KEM_SUITE,
        b"shared_secret",
        &[enc, pk_r],
        &mut shared_secret,
    );
    shared_secret
}

/// The base-mode key schedule (section 5.1): the AEAD key and base nonce.
fn key_schedule(shared_secret: &[u8; 32], info: &[u8]) -> ([u8; 16], [u8; 12]) {
    // No PSK in base mode, so its id is empty.
    let (psk_id_hash, _) = labeled_extract(HPKE_SUITE, &[], b"psk_id_hash", &[]);
    let (info_hash, _) = labeled_extract(HPKE_SUITE, &[], b"info_hash", info);
    let context: [&[u8]; 3] = [&[MODE_BASE], &psk_id_hash, &info_hash];
    let (_, secret) = labeled_extract(HPKE_SUITE, shared_secret, b"secret", &[]);
    let mut key = [0; 16];
    let mut nonce = [0; 12];
    labeled_expand(&secret, HPKE_SUITE, b"key", &context, &mut key);
    labeled_expand(&secret, HPKE_SUITE, b"base_nonce", &context, &mut nonce);
    (key, nonce)
}

/// `LabeledExtract(salt, label, ikm)`: the PRK, and HKDF ready to expand it.
fn labeled_extract(
    suite: &[u8],
    salt: &[u8],
    label: &[u8],
    ikm: &[u8],
) -> ([u8; 32], Hkdf<Sha256>) {
    let mut extract = HkdfExtract::<Sha256>::new(Some(salt));
    for part in [b"HPKE-v1", suite, label, ikm] {
        extract.input_ikm(part);
    }
    let (prk, hkdf) = extract.finalize();
    (prk.into(), hkdf)
}

/// `LabeledExpand(prk, label, info, L)`, with `info` given in up to 3 parts and `L = okm.len()`.
fn labeled_expand(prk: &Hkdf<Sha256>, suite: &[u8], label: &[u8], info: &[&[u8]], okm: &mut [u8]) {
    let len = (okm.len() as u16).to_be_bytes();
    let mut parts: [&[u8]; 7] = [&len, b"HPKE-v1", suite, label, &[], &[], &[]];
    parts[4..4 + info.len()].copy_from_slice(info);
    // Only fails for more than 255 * 32 bytes; these are at most 32.
    prk.expand_multi_info(&parts[..4 + info.len()], okm)
        .expect("short HKDF output");
}
