//! `SpiInterface` framing: the header bytes, wait states and 64-byte chunks, checked against the
//! PTP spec's SPI protocol (section 6.4).

use embedded_hal_mock::eh1::digital::{Mock as PinMock, State, Transaction as Pin};
use embedded_hal_mock::eh1::spi::{Mock as SpiMock, Transaction as Spi};
use tpm::Interface;
use tpm::interface::SpiInterface;
use tpm::interface::spi::{MAX_WAIT_STATES, SpiError};

/// CS# low then high, once per transfer.
fn cs(transfers: usize) -> PinMock {
    let pulses: Vec<Pin> = (0..transfers)
        .flat_map(|_| [Pin::set(State::Low), Pin::set(State::High)])
        .collect();
    PinMock::new(&pulses)
}

/// The header for `len` bytes at `addr`, with the TPM's reply on the last byte.
fn header(read: bool, len: usize, addr: u16, last_miso: u8) -> Spi<u8> {
    let first = if read { 0x80 } else { 0x00 } | (len - 1) as u8;
    let [hi, lo] = addr.to_be_bytes();
    Spi::transfer_in_place(vec![first, 0xD4, hi, lo], vec![0, 0, 0, last_miso])
}

fn check(iface: SpiInterface<SpiMock<u8>, PinMock>) {
    let (mut spi, mut cs) = iface.release();
    spi.done();
    cs.done();
}

#[tokio::test]
async fn read_sends_header_then_reads() {
    let spi = SpiMock::new(&[
        header(true, 4, 0x0F00, 0x01),
        Spi::read_vec(vec![0xD1, 0x15, 0x1B, 0x00]),
        Spi::flush(),
    ]);
    let mut iface = SpiInterface::new(spi, cs(1));
    let mut data = [0; 4];
    iface.read(0x0F00, &mut data).await.unwrap();
    assert_eq!(data, [0xD1, 0x15, 0x1B, 0x00]);
    check(iface);
}

#[tokio::test]
async fn write_sends_header_then_data() {
    let spi = SpiMock::new(&[
        header(false, 1, 0x0000, 0x01),
        Spi::write_vec(vec![0x02]),
        Spi::flush(),
    ]);
    let mut iface = SpiInterface::new(spi, cs(1));
    iface.write(0x0000, &[0x02]).await.unwrap();
    check(iface);
}

#[tokio::test]
async fn waits_until_tpm_is_ready() {
    let spi = SpiMock::new(&[
        header(true, 1, 0x0000, 0x00),
        Spi::read(0x00),
        Spi::read(0x01),
        Spi::read_vec(vec![0xA1]),
        Spi::flush(),
    ]);
    let mut iface = SpiInterface::new(spi, cs(1));
    let mut data = [0];
    iface.read(0x0000, &mut data).await.unwrap();
    assert_eq!(data, [0xA1]);
    check(iface);
}

#[tokio::test]
async fn gives_up_after_max_wait_states_and_releases_cs() {
    let mut expected = vec![header(true, 1, 0x0000, 0x00)];
    expected.extend((0..MAX_WAIT_STATES).map(|_| Spi::read(0x00)));
    let mut iface = SpiInterface::new(SpiMock::new(&expected), cs(1));
    let mut data = [0];
    let result = iface.read(0x0000, &mut data).await;
    assert!(matches!(result, Err(SpiError::WaitState)), "{result:?}");
    check(iface);
}

#[tokio::test]
async fn splits_long_transfers_into_64_byte_frames() {
    let fifo = 0x0024;
    let data: Vec<u8> = (0..100).collect();
    let spi = SpiMock::new(&[
        header(false, 64, fifo, 0x01),
        Spi::write_vec(data[..64].to_vec()),
        Spi::flush(),
        header(false, 36, fifo, 0x01),
        Spi::write_vec(data[64..].to_vec()),
        Spi::flush(),
    ]);
    let mut iface = SpiInterface::new(spi, cs(2));
    iface.write(fifo, &data).await.unwrap();
    check(iface);
}
