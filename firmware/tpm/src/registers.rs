//! TPM interface registers (PTP spec, section 6.3 "FIFO Interface").
//!
//! Each locality has its own copy of the registers at `locality << 12`. These are locality 0's,
//! the only one this driver uses.

use bitflags::bitflags;

/// Locality control and status, 1 byte. See [`Access`].
pub const ACCESS: u16 = 0x0000;
/// Vendor ID (bits 15..0) and device ID (bits 31..16), 4 bytes, little-endian.
pub const DID_VID: u16 = 0x0F00;
/// Revision ID, 1 byte.
pub const RID: u16 = 0x0F04;

bitflags! {
    /// `TPM_ACCESS` bits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Access: u8 {
        /// The other bits are valid.
        const VALID = 0x80;
        /// Read: this locality is active. Write: give it up.
        const ACTIVE_LOCALITY = 0x20;
        /// Write: request this locality.
        const REQUEST_USE = 0x02;
    }
}
