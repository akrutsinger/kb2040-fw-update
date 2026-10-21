//! `Tpm` register logic, against a fake register file instead of a bus.

use std::collections::VecDeque;

use embedded_hal_mock::eh1::delay::NoopDelay;
use tpm::registers::{ACCESS, DID_VID, RID};
use tpm::{Error, Id, Interface, Tpm};

/// Answers reads from fixed values, and `ACCESS` reads from a queue. Records writes.
#[derive(Default)]
struct FakeTpm {
    access: VecDeque<u8>,
    did_vid: [u8; 4],
    rid: u8,
    writes: Vec<(u16, Vec<u8>)>,
}

impl Interface for &mut FakeTpm {
    type Error = ();

    async fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), ()> {
        self.writes.push((addr, data.to_vec()));
        Ok(())
    }

    async fn read(&mut self, addr: u16, data: &mut [u8]) -> Result<(), ()> {
        match addr {
            ACCESS => data[0] = self.access.pop_front().expect("unexpected ACCESS read"),
            DID_VID => data.copy_from_slice(&self.did_vid),
            RID => data[0] = self.rid,
            _ => panic!("unexpected read at {addr:#06x}"),
        }
        Ok(())
    }
}

const VALID: u8 = 0x80;
const ACTIVE: u8 = 0x20;
const REQUEST_USE: u8 = 0x02;

#[tokio::test]
async fn id_reads_little_endian_registers() {
    let mut fake = FakeTpm {
        did_vid: [0xD1, 0x15, 0x1B, 0x00],
        rid: 0x16,
        ..Default::default()
    };
    let id = Tpm::new(&mut fake, NoopDelay).id().await.unwrap();
    assert_eq!(
        id,
        Id {
            vid: 0x15D1,
            did: 0x001B,
            rid: 0x16
        }
    );
}

#[tokio::test]
async fn request_locality_writes_request_and_waits_for_active() {
    let mut fake = FakeTpm {
        access: [VALID, VALID, VALID | ACTIVE].into(),
        ..Default::default()
    };
    Tpm::new(&mut fake, NoopDelay)
        .request_locality()
        .await
        .unwrap();
    assert_eq!(fake.writes, [(ACCESS, vec![REQUEST_USE])]);
    assert!(fake.access.is_empty());
}

#[tokio::test]
async fn request_locality_skips_request_when_already_active() {
    let mut fake = FakeTpm {
        access: [VALID | ACTIVE].into(),
        ..Default::default()
    };
    Tpm::new(&mut fake, NoopDelay)
        .request_locality()
        .await
        .unwrap();
    assert!(fake.writes.is_empty());
}

#[tokio::test]
async fn active_bit_without_valid_does_not_count() {
    let mut fake = FakeTpm {
        access: std::iter::repeat_n(ACTIVE, 751).collect(),
        ..Default::default()
    };
    let result = Tpm::new(&mut fake, NoopDelay).request_locality().await;
    assert_eq!(result, Err(Error::LocalityTimeout { access: ACTIVE }));
}
