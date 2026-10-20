//! SPI transport for the TPM registers (PTP spec, section 6.4 "SPI Hardware Protocol").
//!
//! Each transfer, with CS# held low throughout:
//!
//! 1. A 4-byte header: `R/W | (len - 1)`, then the 24-bit address `0xD4_xxxx`.
//! 2. Wait states. While the last header byte goes out, the TPM answers on MISO. Bit 0 clear
//!    means it isn't ready, so the host clocks single bytes until one comes back with bit 0 set.
//! 3. `len` data bytes, 1 to 64.
//!
//! Step 2 is why this takes an [`SpiBus`] plus its own CS pin rather than an `SpiDevice`: a device
//! transaction is a fixed list of operations, so it can't keep clocking for as long as the TPM
//! waits. The Linux driver (`tpm_tis_spi_main.c`) does the same with `spi_bus_lock`.

use core::fmt;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::spi::SpiBus;

use super::Interface;

/// Header bit 7: read. Clear for a write.
const READ: u8 = 0x80;
const WRITE: u8 = 0x00;
/// First address byte of every TPM register (`0xD4_0000` + offset).
const ADDRESS_PREFIX: u8 = 0xD4;
/// Most data bytes in one transfer.
const MAX_TRANSFER: usize = 64;
/// Bit 0 of the byte on MISO: the TPM is ready for the data phase.
const READY: u8 = 0x01;
/// Wait-state bytes to clock before giving up, as in the Linux driver (`TPM_RETRY`).
pub const MAX_WAIT_STATES: usize = 50;

/// Errors from [`SpiInterface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiError<S, P> {
    /// The SPI bus failed.
    Spi(S),
    /// Setting CS# failed.
    Cs(P),
    /// The TPM was still in a wait state after [`MAX_WAIT_STATES`] bytes. Often what a missing or
    /// unselected TPM looks like, since MISO then floats.
    WaitState,
}

impl<S: fmt::Debug, P: fmt::Debug> fmt::Display for SpiError<S, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spi(e) => write!(f, "SPI error: {e:?}"),
            Self::Cs(e) => write!(f, "CS# pin error: {e:?}"),
            Self::WaitState => write!(f, "TPM never ready (missing or miswired?)"),
        }
    }
}

/// TPM registers over SPI.
pub struct SpiInterface<SPI, CS> {
    spi: SPI,
    cs: CS,
}

impl<SPI: SpiBus, CS: OutputPin> SpiInterface<SPI, CS> {
    /// Wraps an SPI bus (mode 0, up to 43 MHz on the SLB 9670) and the CS# pin, which should start
    /// high.
    pub fn new(spi: SPI, cs: CS) -> Self {
        Self { spi, cs }
    }

    /// Releases the bus and the pin.
    pub fn release(self) -> (SPI, CS) {
        (self.spi, self.cs)
    }

    /// Sends the header, then clocks through any wait states. CS# must already be low.
    async fn header(
        &mut self,
        direction: u8,
        addr: u16,
        len: usize,
    ) -> Result<(), SpiError<SPI::Error, CS::Error>> {
        let [addr_hi, addr_lo] = addr.to_be_bytes();
        // `len` is 1..=MAX_TRANSFER, so `len - 1` fits in the 6-bit size field.
        let mut header = [
            direction | (len - 1) as u8,
            ADDRESS_PREFIX,
            addr_hi,
            addr_lo,
        ];
        self.spi
            .transfer_in_place(&mut header)
            .await
            .map_err(SpiError::Spi)?;
        if header[3] & READY != 0 {
            return Ok(());
        }
        for _ in 0..MAX_WAIT_STATES {
            let mut byte = [0];
            self.spi.read(&mut byte).await.map_err(SpiError::Spi)?;
            if byte[0] & READY != 0 {
                return Ok(());
            }
        }
        Err(SpiError::WaitState)
    }

    async fn read_frame(
        &mut self,
        addr: u16,
        data: &mut [u8],
    ) -> Result<(), SpiError<SPI::Error, CS::Error>> {
        self.header(READ, addr, data.len()).await?;
        self.spi.read(data).await.map_err(SpiError::Spi)?;
        self.spi.flush().await.map_err(SpiError::Spi)
    }

    async fn write_frame(
        &mut self,
        addr: u16,
        data: &[u8],
    ) -> Result<(), SpiError<SPI::Error, CS::Error>> {
        self.header(WRITE, addr, data.len()).await?;
        self.spi.write(data).await.map_err(SpiError::Spi)?;
        self.spi.flush().await.map_err(SpiError::Spi)
    }
}

/// Data longer than 64 bytes goes out as several transfers to the same address. That only makes
/// sense for the data FIFO, which is the only register that long.
impl<SPI: SpiBus, CS: OutputPin> Interface for SpiInterface<SPI, CS> {
    type Error = SpiError<SPI::Error, CS::Error>;

    async fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), Self::Error> {
        for chunk in data.chunks(MAX_TRANSFER) {
            self.cs.set_low().map_err(SpiError::Cs)?;
            let result = self.write_frame(addr, chunk).await;
            // Deselect even after an error, so the next transfer starts with a fresh header.
            self.cs.set_high().map_err(SpiError::Cs)?;
            result?;
        }
        Ok(())
    }

    async fn read(&mut self, addr: u16, data: &mut [u8]) -> Result<(), Self::Error> {
        for chunk in data.chunks_mut(MAX_TRANSFER) {
            self.cs.set_low().map_err(SpiError::Cs)?;
            let result = self.read_frame(addr, chunk).await;
            self.cs.set_high().map_err(SpiError::Cs)?;
            result?;
        }
        Ok(())
    }
}
