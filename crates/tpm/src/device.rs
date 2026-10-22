//! The TPM device.

use embedded_hal_async::delay::DelayNs;

use crate::error::Error;
use crate::interface::Interface;
use crate::registers::{ACCESS, Access, DATA_FIFO, DID_VID, RID, STS, Sts};

// PTP spec timeouts (section 6.5.1.4): A for localities and the burst count, B for becoming ready
// for a command, C for status changes while moving data.
const TIMEOUT_A_MS: u32 = 750;
const TIMEOUT_B_MS: u32 = 2000;
const TIMEOUT_C_MS: u32 = 200;
/// How long a command may run. Key generation is the slow case; ECC takes well under a second on
/// the SLB 9670, so this is generous.
const COMMAND_TIMEOUT_MS: u32 = 60_000;
const POLL_MS: u32 = 1;

/// Every command and response starts with tag (2 bytes), size (4) and code (4), big-endian.
pub(crate) const HEADER_LEN: usize = 10;

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

/// A TPM on interface `I`. `D` paces polling and retries.
pub struct Tpm<I, D> {
    iface: I,
    pub(crate) delay: D,
}

impl<I: Interface, D: DelayNs> Tpm<I, D> {
    /// A TPM on `iface`, polled with `delay`.
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

    /// Runs one TPM2 command and reads its response into `response`. Returns the response length.
    ///
    /// The command must be complete, header included. The response is returned whatever its
    /// response code; [`crate::command`] checks it.
    pub async fn execute(
        &mut self,
        command: &[u8],
        response: &mut [u8],
    ) -> Result<usize, Error<I::Error>> {
        self.request_locality().await?;
        let result = self.execute_inner(command, response).await;
        // Back to idle. After an error this also aborts the command and drops any response left
        // in the FIFO, so the next command starts clean.
        let idle = self.write_sts(Sts::COMMAND_READY).await;
        let len = result?;
        idle?;
        Ok(len)
    }

    /// Reads any register directly, for debugging. `addr` is as in [`crate::registers`].
    pub async fn read_raw(&mut self, addr: u16, data: &mut [u8]) -> Result<(), Error<I::Error>> {
        self.iface.read(addr, data).await.map_err(Error::Bus)
    }

    /// The FIFO protocol, PTP spec section 5.5.2.
    async fn execute_inner(
        &mut self,
        command: &[u8],
        response: &mut [u8],
    ) -> Result<usize, Error<I::Error>> {
        if !self.status().await?.0.contains(Sts::COMMAND_READY) {
            self.write_sts(Sts::COMMAND_READY).await?;
            self.wait_for(Sts::COMMAND_READY, TIMEOUT_B_MS).await?;
        }
        self.send(command).await?;
        self.write_sts(Sts::GO).await?;
        self.wait_for(Sts::VALID | Sts::DATA_AVAIL, COMMAND_TIMEOUT_MS)
            .await?;
        self.receive(response).await
    }

    /// Writes the command into the FIFO, in pieces no larger than the burst count.
    async fn send(&mut self, command: &[u8]) -> Result<(), Error<I::Error>> {
        let Some((last, body)) = command.split_last() else {
            return Err(Error::Fifo);
        };
        let mut sent = 0;
        while sent < body.len() {
            let n = self.burst_count().await?.min(body.len() - sent);
            self.iface
                .write(DATA_FIFO, &body[sent..sent + n])
                .await
                .map_err(Error::Bus)?;
            sent += n;
            // The last byte is still to come, so the TPM must expect more.
            if !self
                .wait_for(Sts::VALID, TIMEOUT_C_MS)
                .await?
                .contains(Sts::EXPECT)
            {
                return Err(Error::Fifo);
            }
        }
        self.iface
            .write(DATA_FIFO, &[*last])
            .await
            .map_err(Error::Bus)?;
        if self
            .wait_for(Sts::VALID, TIMEOUT_C_MS)
            .await?
            .contains(Sts::EXPECT)
        {
            return Err(Error::Fifo);
        }
        Ok(())
    }

    /// Reads the response: the header first, for the size, then the rest.
    async fn receive(&mut self, response: &mut [u8]) -> Result<usize, Error<I::Error>> {
        let header = response
            .get_mut(..HEADER_LEN)
            .ok_or(Error::ResponseTooLarge { needed: HEADER_LEN })?;
        self.read_fifo(header).await?;
        let size = u32::from_be_bytes([header[2], header[3], header[4], header[5]]) as usize;
        if size < HEADER_LEN {
            return Err(Error::Fifo);
        }
        let rest = response
            .get_mut(HEADER_LEN..size)
            .ok_or(Error::ResponseTooLarge { needed: size })?;
        self.read_fifo(rest).await?;
        if self
            .wait_for(Sts::VALID, TIMEOUT_C_MS)
            .await?
            .contains(Sts::DATA_AVAIL)
        {
            return Err(Error::Fifo);
        }
        Ok(size)
    }

    async fn read_fifo(&mut self, buf: &mut [u8]) -> Result<(), Error<I::Error>> {
        let mut got = 0;
        while got < buf.len() {
            self.wait_for(Sts::VALID | Sts::DATA_AVAIL, TIMEOUT_C_MS)
                .await?;
            let n = self.burst_count().await?.min(buf.len() - got);
            self.iface
                .read(DATA_FIFO, &mut buf[got..got + n])
                .await
                .map_err(Error::Bus)?;
            got += n;
        }
        Ok(())
    }

    /// How many bytes the FIFO can move now. Waits while it's 0.
    async fn burst_count(&mut self) -> Result<usize, Error<I::Error>> {
        let mut sts = Sts::empty();
        for _ in 0..TIMEOUT_A_MS / POLL_MS {
            let burst;
            (sts, burst) = self.status().await?;
            if burst > 0 {
                return Ok(burst);
            }
            self.delay.delay_ms(POLL_MS).await;
        }
        Err(Error::StatusTimeout { sts: sts.bits() })
    }

    /// Polls `TPM_STS` until all of `bits` are set, and returns it.
    async fn wait_for(&mut self, bits: Sts, timeout_ms: u32) -> Result<Sts, Error<I::Error>> {
        let mut sts = Sts::empty();
        for _ in 0..timeout_ms / POLL_MS {
            sts = self.status().await?.0;
            if sts.contains(bits) {
                return Ok(sts);
            }
            self.delay.delay_ms(POLL_MS).await;
        }
        Err(Error::StatusTimeout { sts: sts.bits() })
    }

    /// `TPM_STS` and the burst count.
    async fn status(&mut self) -> Result<(Sts, usize), Error<I::Error>> {
        let mut sts = [0; 4];
        self.iface.read(STS, &mut sts).await.map_err(Error::Bus)?;
        let burst = u16::from_le_bytes([sts[1], sts[2]]);
        Ok((Sts::from_bits_retain(sts[0]), burst.into()))
    }

    async fn write_sts(&mut self, sts: Sts) -> Result<(), Error<I::Error>> {
        self.iface
            .write(STS, &[sts.bits()])
            .await
            .map_err(Error::Bus)
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
