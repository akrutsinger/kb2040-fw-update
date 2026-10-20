//! The TPM device.

use embedded_hal_async::delay::DelayNs;

use crate::error::Error;
use crate::interface::Interface;
use crate::registers::{ACCESS, Access, DID_VID, RID};

/// PTP spec `TIMEOUT_A`: how long the TPM may take to grant a locality.
const TIMEOUT_A_MS: u32 = 750;
const POLL_MS: u32 = 1;

/// Identity registers: which chip this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Id {
    /// TCG vendor ID, `0x15D1` for Infineon.
    pub vid: u16,
    /// Vendor's device ID.
    pub did: u16,
    /// Revision ID.
    pub rid: u8,
}

/// A TPM on interface `I`. `D` paces polling.
pub struct Tpm<I, D> {
    iface: I,
    delay: D,
}

impl<I: Interface, D: DelayNs> Tpm<I, D> {
    pub fn new(iface: I, delay: D) -> Self {
        Self { iface, delay }
    }

    /// Makes locality 0 active, which commands need. Does nothing if it already is.
    pub async fn request_locality(&mut self) -> Result<(), Error<I::Error>> {
        if is_active(self.access().await?) {
            return Ok(());
        }
        self.iface
            .write(ACCESS, &[Access::REQUEST_USE.bits()])
            .await
            .map_err(Error::Bus)?;
        let mut access = Access::empty();
        for _ in 0..TIMEOUT_A_MS / POLL_MS {
            access = self.access().await?;
            if is_active(access) {
                return Ok(());
            }
            self.delay.delay_ms(POLL_MS).await;
        }
        Err(Error::LocalityTimeout {
            access: access.bits(),
        })
    }

    /// Reads the vendor, device and revision IDs.
    pub async fn id(&mut self) -> Result<Id, Error<I::Error>> {
        let mut did_vid = [0; 4];
        self.iface
            .read(DID_VID, &mut did_vid)
            .await
            .map_err(Error::Bus)?;
        let mut rid = [0];
        self.iface.read(RID, &mut rid).await.map_err(Error::Bus)?;
        let [vid_lo, vid_hi, did_lo, did_hi] = did_vid;
        Ok(Id {
            vid: u16::from_le_bytes([vid_lo, vid_hi]),
            did: u16::from_le_bytes([did_lo, did_hi]),
            rid: rid[0],
        })
    }

    /// Reads any register directly, for debugging. `addr` is as in [`crate::registers`].
    pub async fn read_raw(&mut self, addr: u16, data: &mut [u8]) -> Result<(), Error<I::Error>> {
        self.iface.read(addr, data).await.map_err(Error::Bus)
    }

    async fn access(&mut self) -> Result<Access, Error<I::Error>> {
        let mut access = [0];
        self.iface
            .read(ACCESS, &mut access)
            .await
            .map_err(Error::Bus)?;
        Ok(Access::from_bits_retain(access[0]))
    }
}

fn is_active(access: Access) -> bool {
    // Without VALID the other bits mean nothing yet.
    access.contains(Access::VALID | Access::ACTIVE_LOCALITY)
}
