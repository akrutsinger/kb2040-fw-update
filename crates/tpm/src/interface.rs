//! Register access to the TPM, independent of the bus.
//!
//! [`Interface`] decouples the driver from the bus, so an I²C transport could sit alongside
//! [`SpiInterface`].

pub mod spi;
pub use spi::SpiInterface;

/// Reads and writes TPM interface registers.
///
/// `addr` is an offset in the TPM's register space, as in [`crate::registers`]. A multi-byte
/// access covers consecutive bytes of one register (all 4 bytes of `DID_VID`, for example), or
/// streams bytes through the data FIFO.
pub trait Interface {
    /// Bus error.
    type Error;

    /// Writes `data` to the register at `addr`.
    async fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), Self::Error>;

    /// Reads the register at `addr` into `data`.
    async fn read(&mut self, addr: u16, data: &mut [u8]) -> Result<(), Self::Error>;
}
