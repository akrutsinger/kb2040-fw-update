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
    Tpm2bPublic, TpmAlgId, TpmEccCurve, TpmRc, TpmRcBase, TpmRh, TpmSt, TpmSu, TpmaObject,
    TpmsAuthCommand, TpmsEccParms, TpmsEccPoint, TpmsSchemeHash, TpmtEccScheme, TpmtKdfScheme,
    TpmtPublic, TpmtSymDefObject, TpmuAsymScheme, TpmuPublicId, TpmuPublicParms,
};
use tpm2_protocol::frame::{
    TpmCreatePrimaryCommand, TpmCreatePrimaryResponse, TpmEvictControlCommand,
    TpmEvictControlResponse, TpmFlushContextCommand, TpmFlushContextResponse, TpmFrame,
    TpmReadPublicCommand, TpmReadPublicResponse, TpmResponse, TpmStartupCommand,
    TpmStartupResponse, TpmUnmarshalBody, tpm_marshal_command,
};

use crate::device::Tpm;
use crate::error::Error;
use crate::interface::Interface;

/// Big enough for every command sent here. CreatePrimary's is the largest, 65 bytes.
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

    /// TPM2_CreatePrimary: a P-256 ECDSA-SHA256 signing key in the owner hierarchy. Returns its
    /// transient handle and public key.
    ///
    /// Primary keys are derived from the hierarchy's seed, so the same template gives the same key
    /// every time, until the TPM is cleared.
    pub async fn create_signing_key(&mut self) -> Result<(u32, PublicKey), Error<I::Error>> {
        let command = TpmCreatePrimaryCommand {
            handles: [owner()],
            in_public: Tpm2bPublic {
                inner: signing_template(),
            },
            // No auth value or data for the key, no outside info, no PCRs.
            ..Default::default()
        };
        let response: TpmCreatePrimaryResponse = self.run(&command, &[password()]).await?;
        let [handle] = response.handles;
        Ok((
            handle.value(),
            parse_signing_key(response.out_public.inner)?,
        ))
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

    /// TPM2_ReadPublic on a key made by [`create_signing_key`](Self::create_signing_key).
    ///
    /// `None` if `handle` is empty. [`Error::UnexpectedKey`] if it holds a key made from a
    /// different template, so a stray key is never mistaken for ours.
    pub async fn read_signing_key(
        &mut self,
        handle: u32,
    ) -> Result<Option<PublicKey>, Error<I::Error>> {
        let command = TpmReadPublicCommand {
            handles: [TpmHandle::new(handle)],
        };
        match self.run::<_, TpmReadPublicResponse>(&command, &[]).await {
            Ok(response) => parse_signing_key(response.out_public.inner).map(Some),
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
        let response = TpmResponse::cast(&response_buf[..len]).map_err(Error::Protocol)?;
        match response.rc().map_err(Error::Protocol)? {
            TpmRc::Fmt0(TpmRcBase::Success) => response.unmarshal().map_err(Error::Protocol),
            rc => Err(Error::Tpm(rc)),
        }
    }
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

/// The signing key's public area, with `unique` empty for the TPM to fill in.
fn signing_template() -> TpmtPublic {
    TpmtPublic {
        object_type: TpmAlgId::Ecc,
        name_alg: TpmAlgId::Sha256,
        // Unrestricted, so it signs any digest.
        object_attributes: TpmaObject::FIXED_TPM
            | TpmaObject::FIXED_PARENT
            | TpmaObject::SENSITIVE_DATA_ORIGIN
            | TpmaObject::USER_WITH_AUTH
            | TpmaObject::NO_DA
            | TpmaObject::SIGN_ENCRYPT,
        auth_policy: Default::default(),
        parameters: TpmuPublicParms::Ecc(TpmsEccParms {
            // Not a storage key, so no symmetric algorithm.
            symmetric: TpmtSymDefObject {
                algorithm: TpmAlgId::Null,
                ..Default::default()
            },
            scheme: TpmtEccScheme {
                scheme: TpmAlgId::Ecdsa,
                details: TpmuAsymScheme::Hash(TpmsSchemeHash {
                    hash_alg: TpmAlgId::Sha256,
                }),
            },
            curve_id: TpmEccCurve::NistP256,
            kdf: TpmtKdfScheme::default(),
        }),
        unique: TpmuPublicId::Ecc(TpmsEccPoint::default()),
    }
}

/// The key from a public area, which must match [`signing_template`] apart from `unique`.
fn parse_signing_key<E>(mut public: TpmtPublic) -> Result<PublicKey, Error<E>> {
    let template = signing_template();
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
