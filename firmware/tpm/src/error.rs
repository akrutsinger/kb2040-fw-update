//! Driver error type.

use core::fmt;

use tpm2_protocol::TpmError;
use tpm2_protocol::data::TpmRc;

/// Errors from [`Tpm`](crate::Tpm). `E` is the [`Interface`](crate::Interface)'s error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The bus failed, or the TPM didn't answer on it.
    Bus(E),
    /// The TPM didn't make locality 0 active within `TIMEOUT_A` (750 ms). `access` is the last
    /// `TPM_ACCESS` value read, see [`Access`](crate::registers::Access).
    LocalityTimeout { access: u8 },
    /// `TPM_STS` didn't reach the expected state in time. `sts` is the last value read, see
    /// [`Sts`](crate::registers::Sts).
    StatusTimeout { sts: u8 },
    /// The TPM wanted more or fewer command bytes than the command's header says, sent more
    /// response bytes than its header says, or sent a header with an impossible size.
    Fifo,
    /// The response doesn't fit the buffer. `needed` is the size from its header.
    ResponseTooLarge { needed: usize },
    /// A command couldn't be encoded, or a response decoded (`tpm2-protocol`'s error).
    Protocol(TpmError),
    /// The TPM returned this response code (TPM 2.0 spec part 2, section 6.6).
    Tpm(TpmRc),
    /// The persistent handle holds a key, but not one made from this driver's template.
    UnexpectedKey,
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(e) => e.fmt(f),
            Self::LocalityTimeout { access } => {
                write!(f, "locality 0 not granted, ACCESS {access:#04x}")
            }
            Self::StatusTimeout { sts } => write!(f, "TPM status stuck at {sts:#04x}"),
            Self::Fifo => write!(f, "TPM FIFO out of step with the command"),
            Self::ResponseTooLarge { needed } => write!(f, "TPM response too large ({needed} B)"),
            Self::Protocol(e) => write!(f, "bad TPM message: {e}"),
            Self::Tpm(rc) => write!(f, "TPM error {rc}"),
            Self::UnexpectedKey => write!(f, "a different key is at that handle"),
        }
    }
}
