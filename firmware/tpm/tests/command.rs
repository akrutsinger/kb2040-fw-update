//! TPM2 commands through the FIFO transport, against an emulated FIFO.
//!
//! `FifoTpm` emulates the PTP FIFO registers (`ACCESS`, `STS`, `DATA_FIFO`) and hands each finished
//! command to a backend. The backend is either canned bytes, or swtpm, a real TPM 2.0
//! implementation (the `#[ignore]`d test, run with `--include-ignored`; needs `swtpm` installed).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, SystemTime};

use embedded_hal_mock::eh1::delay::NoopDelay;
use p256::ecdsa::signature::hazmat::PrehashVerifier;
use p256::ecdsa::{Signature as EcdsaSignature, VerifyingKey};
use tpm::registers::{ACCESS, DATA_FIFO, STS};
use tpm::tpm2_protocol::data::TpmRcBase;
use tpm::{Interface, Tpm};

/// Small, so commands and responses take several bursts.
const BURST: usize = 8;

const VALID: u8 = 0x80;
const COMMAND_READY: u8 = 0x40;
const GO: u8 = 0x20;
const DATA_AVAIL: u8 = 0x10;
const EXPECT: u8 = 0x08;

struct FifoTpm<B> {
    backend: B,
    ready: bool,
    command: Vec<u8>,
    response: VecDeque<u8>,
}

impl<B: FnMut(&[u8]) -> Vec<u8>> FifoTpm<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            ready: false,
            command: Vec::new(),
            response: VecDeque::new(),
        }
    }

    /// True while the command is shorter than the size in its header.
    fn expecting(&self) -> bool {
        match self.command.get(2..6) {
            Some(size) => {
                self.command.len() < u32::from_be_bytes(size.try_into().unwrap()) as usize
            }
            None => !self.command.is_empty(),
        }
    }
}

impl<B: FnMut(&[u8]) -> Vec<u8>> Interface for FifoTpm<B> {
    type Error = ();

    async fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), ()> {
        match addr {
            ACCESS => {}
            STS if data[0] == COMMAND_READY => {
                self.ready = true;
                self.command.clear();
                self.response.clear();
            }
            STS if data[0] == GO => {
                assert!(!self.expecting(), "GO before the command was complete");
                self.response = (self.backend)(&self.command).into();
                self.ready = false;
            }
            DATA_FIFO => {
                assert!(self.ready, "command written before commandReady");
                assert!(
                    data.len() <= BURST,
                    "wrote {} bytes, burst is {BURST}",
                    data.len()
                );
                self.command.extend_from_slice(data);
            }
            _ => panic!("unexpected write {data:02x?} at {addr:#06x}"),
        }
        Ok(())
    }

    async fn read(&mut self, addr: u16, data: &mut [u8]) -> Result<(), ()> {
        match addr {
            // Valid, locality 0 active.
            ACCESS => data[0] = 0xA1,
            STS => {
                let mut sts = VALID;
                if self.ready && self.command.is_empty() {
                    sts |= COMMAND_READY;
                }
                if self.expecting() {
                    sts |= EXPECT;
                }
                if !self.response.is_empty() {
                    sts |= DATA_AVAIL;
                }
                let [lo, hi] = (BURST as u16).to_le_bytes();
                let n = data.len();
                data.copy_from_slice(&[sts, lo, hi, 0][..n]);
            }
            DATA_FIFO => {
                assert!(
                    data.len() <= BURST,
                    "read {} bytes, burst is {BURST}",
                    data.len()
                );
                for b in data {
                    *b = self.response.pop_front().expect("read past the response");
                }
            }
            _ => panic!("unexpected read at {addr:#06x}"),
        }
        Ok(())
    }
}

#[tokio::test]
async fn startup_sends_the_spec_bytes_and_accepts_already_started() {
    let backend = |command: &[u8]| {
        // TPM2_Startup(TPM_SU_CLEAR), as in the spec and every TPM log.
        assert_eq!(command, [0x80, 0x01, 0, 0, 0, 0x0C, 0, 0, 0x01, 0x44, 0, 0]);
        // TPM_RC_INITIALIZE: already started.
        vec![0x80, 0x01, 0, 0, 0, 0x0A, 0, 0, 0x01, 0x00]
    };
    let mut tpm = Tpm::new(FifoTpm::new(backend), NoopDelay);
    tpm.startup().await.unwrap();
}

#[tokio::test]
async fn tpm_1_2_firmware_is_recognized() {
    let backend = |_: &[u8]| {
        // What an SLB 9670 with TPM 1.2 firmware answers to TPM2_Startup: tag TPM_ST_RSP_COMMAND,
        // TPM_BADTAG.
        vec![0x00, 0xC4, 0, 0, 0, 0x0A, 0, 0, 0, 0x1E]
    };
    let mut tpm = Tpm::new(FifoTpm::new(backend), NoopDelay);
    assert_eq!(tpm.startup().await, Err(tpm::Error::Tpm12));
}

#[tokio::test]
async fn tpm_errors_come_back_as_response_codes() {
    // TPM_RC_FAILURE.
    let backend = |_: &[u8]| vec![0x80, 0x01, 0, 0, 0, 0x0A, 0, 0, 0x01, 0x01];
    let mut tpm = Tpm::new(FifoTpm::new(backend), NoopDelay);
    let result = tpm.flush(0x8000_0000).await;
    assert!(
        matches!(result, Err(tpm::Error::Tpm(rc)) if rc.base() == TpmRcBase::Failure),
        "{result:?}"
    );
}

/// A swtpm process with a fresh state directory, killed on drop.
struct Swtpm {
    child: Child,
    dir: PathBuf,
}

impl Swtpm {
    fn start() -> (Self, UnixStream) {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tpm-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let sock = dir.join("sock");
        let child = Command::new("swtpm")
            .args(["socket", "--tpm2", "--flags", "not-need-init"])
            .arg("--tpmstate")
            .arg(format!("dir={}", dir.display()))
            .arg("--server")
            .arg(format!("type=unixio,path={}", sock.display()))
            .spawn()
            .expect("swtpm not installed?");
        let swtpm = Self { child, dir };
        for _ in 0..100 {
            if let Ok(stream) = UnixStream::connect(&sock) {
                return (swtpm, stream);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("swtpm didn't open {}", sock.display());
    }
}

impl Drop for Swtpm {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Sends a command to swtpm and returns its response.
fn exchange(stream: &mut UnixStream, command: &[u8]) -> Vec<u8> {
    stream.write_all(command).unwrap();
    let mut response = vec![0; 10];
    stream.read_exact(&mut response).unwrap();
    let size = u32::from_be_bytes(response[2..6].try_into().unwrap()) as usize;
    response.resize(size, 0);
    stream.read_exact(&mut response[10..]).unwrap();
    response
}

#[tokio::test]
#[ignore = "needs swtpm; run with --include-ignored"]
async fn identity_key_flow_against_swtpm() {
    const PERSISTENT: u32 = 0x8100_0002;
    let (_swtpm, mut stream) = Swtpm::start();
    let mut tpm = Tpm::new(FifoTpm::new(|c: &[u8]| exchange(&mut stream, c)), NoopDelay);

    tpm.startup().await.unwrap();
    // A second startup gets TPM_RC_INITIALIZE, which must count as success.
    tpm.startup().await.unwrap();
    assert_eq!(tpm.read_signing_key(PERSISTENT).await, Ok(None));

    let (transient, created) = tpm.create_signing_key().await.unwrap();
    assert_eq!(tpm.read_signing_key(transient).await, Ok(Some(created)));
    tpm.persist(transient, PERSISTENT).await.unwrap();
    tpm.flush(transient).await.unwrap();

    let persisted = tpm.read_signing_key(PERSISTENT).await.unwrap().unwrap();
    assert_eq!(persisted, created);
    p256::PublicKey::from_sec1_bytes(&persisted.to_sec1()).expect("not a P-256 point");

    // Primary keys come from the hierarchy seed: the same template gives the same key.
    let (again, key) = tpm.create_signing_key().await.unwrap();
    assert_eq!(key, persisted);
    tpm.flush(again).await.unwrap();

    // The persisted key signs, and p256 accepts the signature for that digest only.
    let digest = [0x5A; 32];
    let signature = tpm.sign(PERSISTENT, &digest).await.unwrap();
    let verifier = VerifyingKey::from_sec1_bytes(&persisted.to_sec1()).unwrap();
    let signature = EcdsaSignature::from_scalars(signature.r, signature.s).unwrap();
    verifier
        .verify_prehash(&digest, &signature)
        .expect("bad signature");
    assert!(verifier.verify_prehash(&[0xA5; 32], &signature).is_err());
}
