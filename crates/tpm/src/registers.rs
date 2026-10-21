//! TPM interface registers (PTP spec, section 6.3 "FIFO Interface").
//!
//! Each locality has its own copy of the registers at `locality << 12`. These are locality 0's,
//! the only one this driver uses.

use bitflags::bitflags;

/// Locality control and status, 1 byte. See [`Access`].
pub const ACCESS: u16 = 0x0000;
/// Status and burst count, 4 bytes. Byte 0 is [`Sts`], bytes 1..2 the burst count (little-endian):
/// how many bytes the FIFO can take or give right now.
pub const STS: u16 = 0x0018;
/// Data FIFO for commands and responses. Any length; every byte goes to the same address.
pub const DATA_FIFO: u16 = 0x0024;
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

bitflags! {
    /// `TPM_STS` byte 0.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Sts: u8 {
        /// `DATA_AVAIL` and `EXPECT` are valid.
        const VALID = 0x80;
        /// Read: ready for a command. Write: get ready, or abort the current command.
        const COMMAND_READY = 0x40;
        /// Write: run the command in the FIFO.
        const GO = 0x20;
        /// The response has bytes left to read.
        const DATA_AVAIL = 0x10;
        /// The command isn't complete yet.
        const EXPECT = 0x08;
    }
}
