//! Update tickets: what the update server sends with an encrypted release, so that only this board
//! can decrypt it (`offer` in `worker/src/index.ts`).
//!
//! A ticket is `content key (16) || IV (16) || size (4, little-endian) || nonce (32) || version`,
//! sealed with HPKE to the decryption key, with `TICKET_INFO || device id` as `info`. The device id
//! is the first 16 bytes of SHA-256(identity key), as the server names boards.

use core::fmt;

use sha2::{Digest, Sha256};
use tpm::{KeyKind, PublicKey};

use crate::identity::{self, NONCE_LEN};
use crate::{Tpm, TpmError};

/// Must match `TICKET_INFO` in `worker/src/index.ts`.
const TICKET_INFO: &[u8] = b"kb2040-fw-update/ticket/v1";

/// An opened ticket: how to decrypt the release.
pub struct Ticket {
    /// AES-128 content key.
    pub key: [u8; 16],
    /// AES-128-CTR initial counter block.
    pub iv: [u8; 16],
    /// Bytes in the encrypted release: the image and its 64-byte signature.
    pub size: u32,
}

pub enum Error {
    /// `enc` isn't a SEC1 uncompressed point.
    BadEnc,
    Tpm(TpmError),
    /// The ticket didn't decrypt: it isn't for this board, or it was damaged.
    Open,
    /// The ticket decrypted, but is too short to hold its fields.
    Format,
    /// The ticket answers a different request than this board's last one.
    Nonce,
}

impl From<TpmError> for Error {
    fn from(e: TpmError) -> Self {
        Error::Tpm(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadEnc => write!(f, "ticket's enc isn't a P-256 point"),
            Error::Tpm(e) => write!(f, "opening the ticket: {e}"),
            Error::Open => write!(f, "ticket doesn't open, is it for this board?"),
            Error::Format => write!(f, "ticket too short"),
            Error::Nonce => write!(f, "ticket answers a different request"),
        }
    }
}

/// Opens `sealed` (ciphertext and tag, decrypted in place), with `enc`, the server's one-time
/// public key. `nonce` is the one this board signed in its update request.
pub async fn open(
    tpm: &mut Tpm,
    enc: &[u8; hpke::ENC_LEN],
    sealed: &mut [u8],
    nonce: &[u8; NONCE_LEN],
) -> Result<Ticket, Error> {
    let point = PublicKey::from_sec1(enc).ok_or(Error::BadEnc)?;
    // The TPM refuses points that aren't on the curve.
    let (dh, decrypt) = identity::decryption_dh(tpm, &point).await?;
    let (signing, _) = identity::key(tpm, KeyKind::Signing).await?;

    let mut info = [0; TICKET_INFO.len() + 16];
    info[..TICKET_INFO.len()].copy_from_slice(TICKET_INFO);
    info[TICKET_INFO.len()..].copy_from_slice(&Sha256::digest(signing.to_sec1())[..16]);
    let plaintext =
        hpke::open(&dh, enc, &decrypt.to_sec1(), &info, &[], sealed).map_err(|_| Error::Open)?;

    let (ticket, ticket_nonce) = parse(plaintext).ok_or(Error::Format)?;
    if ticket_nonce != nonce {
        return Err(Error::Nonce);
    }
    Ok(ticket)
}

/// The fields of an opened ticket, and its nonce. The version after them isn't needed.
fn parse(plaintext: &[u8]) -> Option<(Ticket, &[u8; NONCE_LEN])> {
    let (key, rest) = plaintext.split_first_chunk()?;
    let (iv, rest) = rest.split_first_chunk()?;
    let (size, rest) = rest.split_first_chunk()?;
    let (nonce, _version) = rest.split_first_chunk()?;
    let ticket = Ticket {
        key: *key,
        iv: *iv,
        size: u32::from_le_bytes(*size),
    };
    Some((ticket, nonce))
}
