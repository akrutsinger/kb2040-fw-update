//! The TPM2 commands this driver uses (TPM 2.0 spec part 3), on top of
//! [`Tpm::execute`](crate::Tpm::execute). `tpm2-protocol` encodes the commands and decodes the
//! responses.
//!
//! Commands that need authorization use a password session with an empty password, which is what
//! a fresh TPM's owner hierarchy has.

use embedded_hal_async::delay::DelayNs;
use tpm2_protocol::TpmWriter;
use tpm2_protocol::basic::TpmHandle;
use tpm2_protocol::data::{
    Tpm2bDigest, Tpm2bEccParameter, Tpm2bEccPoint, Tpm2bPublic, TpmAlgId, TpmEccCurve, TpmRc,
    TpmRcBase, TpmRh, TpmSt, TpmSu, TpmaObject, TpmsAuthCommand, TpmsEccParms, TpmsEccPoint,
    TpmsSchemeHash, TpmtEccScheme, TpmtKdfScheme, TpmtPublic, TpmtSigScheme, TpmtSymDefObject,
    TpmtTkHashcheck, TpmuAsymScheme, TpmuPublicId, TpmuPublicParms, TpmuSignature,
};
use tpm2_protocol::frame::{
    TpmCreatePrimaryCommand, TpmCreatePrimaryResponse, TpmEcdhZGenCommand, TpmEcdhZGenResponse,
    TpmEvictControlCommand, TpmEvictControlResponse, TpmFlushContextCommand,
    TpmFlushContextResponse, TpmFrame, TpmReadPublicCommand, TpmReadPublicResponse, TpmResponse,
    TpmSignCommand, TpmSignResponse, TpmStartupCommand, TpmStartupResponse, TpmUnmarshalBody,
    tpm_marshal_command,
};

use crate::device::Tpm;
use crate::error::Error;
use crate::interface::Interface;

/// Big enough for every command sent here. ECDH_ZGen's is the largest, 97 bytes.
const COMMAND_LEN: usize = 128;
/// Big enough for every response here. CreatePrimary's is the largest, about 300 bytes.
const RESPONSE_LEN: usize = 512;

/// A P-256 public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey {
    pub x: [u8; 32],
    pub y: [u8; 32],
}

impl PublicKey {
    /// SEC1 uncompressed point: `04 || x || y`.
    pub fn to_sec1(&self) -> [u8; 65] {
        let mut out = [0x04; 65];
        out[1..33].copy_from_slice(&self.x);
        out[33..].copy_from_slice(&self.y);
        out
    }
}

/// An ECDSA P-256 signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub r: [u8; 32],
    pub s: [u8; 32],
}

/// The kinds of key this driver makes: P-256 keys in the owner hierarchy that differ only in what
/// the TPM lets them do. Each kind has its own template, so the two are different keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// Signs with ECDSA-SHA256, see [`Tpm::sign`].
    Signing,
    /// Does ECDH key agreement, see [`Tpm::ecdh_zgen`]. It can't sign.
    Decrypt,
}

impl<I: Interface, D: DelayNs> Tpm<I, D> {
    /// TPM2_Startup(CLEAR). Succeeds if the TPM is already started, which is normal here: it
    /// keeps power when only the MCU resets.
    pub async fn startup(&mut self) -> Result<(), Error<I::Error>> {
        let command = TpmStartupCommand {
            handles: [],
            startup_type: TpmSu::Clear,
        };
        match self.run::<_, TpmStartupResponse>(&command, &[]).await {
            Ok(_) => Ok(()),
            Err(Error::Tpm(rc)) if rc.base() == TpmRcBase::Initialize => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// TPM2_CreatePrimary: a P-256 key of `kind` in the owner hierarchy. Returns its transient
    /// handle and public key.
    ///
    /// Primary keys are derived from the hierarchy's seed, so the same template gives the same key
    /// every time, until the TPM is cleared.
    pub async fn create_key(&mut self, kind: KeyKind) -> Result<(u32, PublicKey), Error<I::Error>> {
        let command = TpmCreatePrimaryCommand {
            handles: [owner()],
            in_public: Tpm2bPublic {
                inner: template(kind),
            },
            // No auth value or data for the key, no outside info, no PCRs.
            ..Default::default()
        };
        let response: TpmCreatePrimaryResponse = self.run(&command, &[password()]).await?;
        let [handle] = response.handles;
        Ok((handle.value(), parse_key(response.out_public.inner, kind)?))
    }

    /// TPM2_EvictControl: copies transient `object` to `persistent`, where it survives power
    /// loss. The transient copy stays loaded, so [`flush`](Self::flush) it.
    pub async fn persist(&mut self, object: u32, persistent: u32) -> Result<(), Error<I::Error>> {
        let command = TpmEvictControlCommand {
            handles: [owner(), TpmHandle::new(object)],
            persistent_handle: TpmHandle::new(persistent),
        };
        self.run::<_, TpmEvictControlResponse>(&command, &[password()])
            .await?;
        Ok(())
    }

    /// TPM2_FlushContext: unloads a transient object.
    pub async fn flush(&mut self, handle: u32) -> Result<(), Error<I::Error>> {
        let command = TpmFlushContextCommand {
            handles: [],
            flush_handle: TpmHandle::new(handle),
        };
        self.run::<_, TpmFlushContextResponse>(&command, &[])
            .await?;
        Ok(())
    }

    /// TPM2_Sign: signs `digest`, a SHA-256 hash, with `key`, a [`KeyKind::Signing`] key.
    pub async fn sign(
        &mut self,
        key: u32,
        digest: &[u8; 32],
    ) -> Result<Signature, Error<I::Error>> {
        let command = TpmSignCommand {
            handles: [TpmHandle::new(key)],
            digest: Tpm2bDigest::try_from(&digest[..]).map_err(Error::Protocol)?,
            // NULL: use the key's own scheme, ECDSA-SHA256.
            in_scheme: TpmtSigScheme::default(),
            // A NULL ticket. Only restricted keys need proof that the TPM made the digest.
            validation: TpmtTkHashcheck {
                tag: TpmSt::HashCheck,
                hierarchy: TpmRh::Null,
                digest: Default::default(),
            },
        };
        let response: TpmSignResponse = self.run(&command, &[password()]).await?;
        let TpmuSignature::Ecdsa(signature) = response.signature.signature else {
            return Err(Error::UnexpectedKey);
        };
        match (
            scalar(&signature.signature_r),
            scalar(&signature.signature_s),
        ) {
            (Some(r), Some(s)) => Ok(Signature { r, s }),
            _ => Err(Error::UnexpectedKey),
        }
    }

    /// TPM2_ECDH_ZGen: multiplies `point` by the private half of `key`, a [`KeyKind::Decrypt`]
    /// key, and returns the result's x-coordinate. That's the ECDH shared secret, the form HPKE
    /// and ECIES use. The private key stays in the TPM.
    ///
    /// The TPM refuses a point that isn't on the curve, with `TPM_RC_ECC_POINT`.
    pub async fn ecdh_zgen(
        &mut self,
        key: u32,
        point: &PublicKey,
    ) -> Result<[u8; 32], Error<I::Error>> {
        let command = TpmEcdhZGenCommand {
            handles: [TpmHandle::new(key)],
            in_point: Tpm2bEccPoint {
                inner: TpmsEccPoint {
                    x: Tpm2bEccParameter::try_from(&point.x[..]).map_err(Error::Protocol)?,
                    y: Tpm2bEccParameter::try_from(&point.y[..]).map_err(Error::Protocol)?,
                },
            },
        };
        let response: TpmEcdhZGenResponse = self.run(&command, &[password()]).await?;
        scalar(&response.out_point.inner.x).ok_or(Error::UnexpectedKey)
    }

    /// TPM2_ReadPublic on a key of `kind`.
    ///
    /// `None` if `handle` is empty. [`Error::UnexpectedKey`] if it holds a key made from a
    /// different template, so a stray key (or one of the other kind) is never mistaken for ours.
    pub async fn read_key(
        &mut self,
        handle: u32,
        kind: KeyKind,
    ) -> Result<Option<PublicKey>, Error<I::Error>> {
        let command = TpmReadPublicCommand {
            handles: [TpmHandle::new(handle)],
        };
        match self.run::<_, TpmReadPublicResponse>(&command, &[]).await {
            Ok(response) => parse_key(response.out_public.inner, kind).map(Some),
            Err(Error::Tpm(rc)) if rc.base() == TpmRcBase::Handle => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Runs a command and decodes its response. A response code other than success comes back
    /// as [`Error::Tpm`].
    async fn run<C: TpmFrame, R: TpmUnmarshalBody>(
        &mut self,
        command: &C,
        sessions: &[TpmsAuthCommand],
    ) -> Result<R, Error<I::Error>> {
        let tag = if sessions.is_empty() {
            TpmSt::NoSessions
        } else {
            TpmSt::Sessions
        };
        let mut command_buf = [0; COMMAND_LEN];
        let mut writer = TpmWriter::new(&mut command_buf);
        tpm_marshal_command(command, tag, sessions, &mut writer).map_err(Error::Protocol)?;
        let command_len = writer.len();

        let mut response_buf = [0; RESPONSE_LEN];
        let len = self
            .execute(&command_buf[..command_len], &mut response_buf)
            .await?;
        // A TPM 1.2 answers TPM 2.0 commands in its own format, tagged TPM_ST_RSP_COMMAND. A TPM 2.0
        // only uses that tag to reject a command with a bad tag, which this driver never sends.
        if u16::from_be_bytes([response_buf[0], response_buf[1]]) == TpmSt::RspCommand as u16 {
            return Err(Error::Tpm12);
        }
        let response = TpmResponse::cast(&response_buf[..len]).map_err(Error::Protocol)?;
        match response.rc().map_err(Error::Protocol)? {
            TpmRc::Fmt0(TpmRcBase::Success) => response.unmarshal().map_err(Error::Protocol),
            rc => Err(Error::Tpm(rc)),
        }
    }
}

/// A P-256 scalar, left-padded to 32 bytes in case the TPM dropped leading zeros.
fn scalar(bytes: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0; 32];
    out.get_mut(32usize.checked_sub(bytes.len())?..)?
        .copy_from_slice(bytes);
    Some(out)
}

fn owner() -> TpmHandle {
    TpmHandle::new(TpmRh::Owner as u32)
}

/// A password session with an empty password.
fn password() -> TpmsAuthCommand {
    TpmsAuthCommand {
        session_handle: TpmHandle::new(TpmRh::Pw as u32),
        ..Default::default()
    }
}

/// The public area of a `kind` key, with `unique` empty for the TPM to fill in.
fn template(kind: KeyKind) -> TpmtPublic {
    let (usage, scheme) = match kind {
        // Unrestricted, so it signs any digest.
        KeyKind::Signing => (
            TpmaObject::SIGN_ENCRYPT,
            TpmtEccScheme {
                scheme: TpmAlgId::Ecdsa,
                details: TpmuAsymScheme::Hash(TpmsSchemeHash {
                    hash_alg: TpmAlgId::Sha256,
                }),
            },
        ),
        // Unrestricted, so ECDH_ZGen takes any point. NULL scheme: ZGen doesn't use one.
        KeyKind::Decrypt => (TpmaObject::DECRYPT, TpmtEccScheme::default()),
    };
    TpmtPublic {
        object_type: TpmAlgId::Ecc,
        name_alg: TpmAlgId::Sha256,
        object_attributes: TpmaObject::FIXED_TPM
            | TpmaObject::FIXED_PARENT
            | TpmaObject::SENSITIVE_DATA_ORIGIN
            | TpmaObject::USER_WITH_AUTH
            | TpmaObject::NO_DA
            | usage,
        auth_policy: Default::default(),
        parameters: TpmuPublicParms::Ecc(TpmsEccParms {
            // Not a storage key, so no symmetric algorithm.
            symmetric: TpmtSymDefObject {
                algorithm: TpmAlgId::Null,
                ..Default::default()
            },
            scheme,
            curve_id: TpmEccCurve::NistP256,
            kdf: TpmtKdfScheme::default(),
        }),
        unique: TpmuPublicId::Ecc(TpmsEccPoint::default()),
    }
}

/// The key from a public area, which must match `kind`'s [`template`] apart from `unique`.
fn parse_key<E>(mut public: TpmtPublic, kind: KeyKind) -> Result<PublicKey, Error<E>> {
    let template = template(kind);
    let unique = core::mem::replace(&mut public.unique, template.unique.clone());
    if public != template {
        return Err(Error::UnexpectedKey);
    }
    let TpmuPublicId::Ecc(point) = unique else {
        return Err(Error::UnexpectedKey);
    };
    match ((*point.x).try_into(), (*point.y).try_into()) {
        (Ok(x), Ok(y)) => Ok(PublicKey { x, y }),
        _ => Err(Error::UnexpectedKey),
    }
}
