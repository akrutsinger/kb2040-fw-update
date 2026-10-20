//! Driver error type.

use core::fmt;

/// Errors from [`Tpm`](crate::Tpm). `E` is the [`Interface`](crate::Interface)'s error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The bus failed, or the TPM didn't answer on it.
    Bus(E),
    /// The TPM didn't make locality 0 active within `TIMEOUT_A` (750 ms). `access` is the last
    /// `TPM_ACCESS` value read, see [`Access`](crate::registers::Access).
    LocalityTimeout { access: u8 },
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(e) => e.fmt(f),
            Self::LocalityTimeout { access } => {
                write!(f, "locality 0 not granted, ACCESS {access:#04x}")
            }
        }
    }
}
