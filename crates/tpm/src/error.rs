//! Driver error type.

use core::fmt;

use tpm2_protocol::TpmError;
use tpm2_protocol::data::TpmRc;

/// Errors from [`Tpm`](crate::Tpm). `E` is the [`Interface`](crate::Interface)'s error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The bus failed, or the TPM didn't answer on it.
    Bus(E),
    /// The TPM didn't make locality 0 active within `TIMEOUT_A` (750 ms).
    LocalityTimeout {
        /// Last `TPM_ACCESS` read, see [`Access`](crate::registers::Access).
        access: u8,
    },
    /// `TPM_STS` didn't reach the expected state in time.
    StatusTimeout {
        /// Last `TPM_STS` read, see [`Sts`](crate::registers::Sts).
        sts: u8,
    },
    /// The TPM wanted more or fewer command bytes than the command's header says, sent more
    /// response bytes than its header says, or sent a header with an impossible size.
    Fifo,
    /// The response doesn't fit the buffer.
    ResponseTooLarge {
        /// Size from the response header.
        needed: usize,
    },
    /// A command couldn't be encoded, or a response decoded (`tpm2-protocol`'s error).
    Protocol(TpmError),
    /// The TPM returned this response code (TPM 2.0 spec part 2, section 6.6).
    Tpm(TpmRc),
    /// The persistent handle holds a key, but not one made from this driver's template.
    UnexpectedKey,
    /// The TPM answered in TPM 1.2 format: it runs TPM 1.2 firmware, which has none of the TPM 2.0
    /// commands. Infineon's SLB 9670 can be switched to TPM 2.0 firmware (see README).
    Tpm12,
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
            Self::Tpm12 => write!(f, "TPM runs TPM 1.2 firmware, needs 2.0 (see README)"),
        }
    }
}

// No `source()`: `Display` already prints the inner error, so a chain would print it twice.
impl<E: fmt::Debug + fmt::Display> core::error::Error for Error<E> {}
