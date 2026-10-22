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
/// Everything before the version.
const FIXED_LEN: usize = 16 + 16 + 4 + NONCE_LEN;

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
    let [0x04, point @ ..] = enc else {
        return Err(Error::BadEnc);
    };
    let point: &[u8] = point;
    let (x, y) = point.split_at(32);
    let point = PublicKey {
        x: x.try_into().unwrap(),
        y: y.try_into().unwrap(),
    };
    // The TPM refuses points that aren't on the curve.
    let (dh, decrypt) = identity::decryption_dh(tpm, &point).await?;
    let (signing, _) = identity::key(tpm, KeyKind::Signing).await?;

    let mut info = [0; TICKET_INFO.len() + 16];
    info[..TICKET_INFO.len()].copy_from_slice(TICKET_INFO);
    info[TICKET_INFO.len()..].copy_from_slice(&Sha256::digest(signing.to_sec1())[..16]);
    let plaintext =
        hpke::open(&dh, enc, &decrypt.to_sec1(), &info, &[], sealed).map_err(|_| Error::Open)?;

    let Some((fixed, _version)) = plaintext.split_at_checked(FIXED_LEN) else {
        return Err(Error::Format);
    };
    let (key, rest) = fixed.split_at(16);
    let (iv, rest) = rest.split_at(16);
    let (size, ticket_nonce) = rest.split_at(4);
    if ticket_nonce != nonce {
        return Err(Error::Nonce);
    }
    Ok(Ticket {
        key: key.try_into().unwrap(),
        iv: iv.try_into().unwrap(),
        size: u32::from_le_bytes(size.try_into().unwrap()),
    })
}
