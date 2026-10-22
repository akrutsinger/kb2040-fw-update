//! `PublicKey`'s SEC1 encoding.

use tpm::PublicKey;

const KEY: PublicKey = PublicKey {
    x: [1; 32],
    y: [2; 32],
};

#[test]
fn sec1_round_trips() {
    assert_eq!(PublicKey::from_sec1(&KEY.to_sec1()), Some(KEY));
}

#[test]
fn from_sec1_rejects_other_encodings_and_lengths() {
    let sec1 = KEY.to_sec1();
    let mut compressed = sec1;
    compressed[0] = 0x02;
    assert_eq!(PublicKey::from_sec1(&compressed), None);
    assert_eq!(PublicKey::from_sec1(&sec1[..64]), None);
    assert_eq!(
        PublicKey::from_sec1(&[sec1.as_slice(), &[0]].concat()),
        None
    );
    assert_eq!(PublicKey::from_sec1(&[]), None);
}
