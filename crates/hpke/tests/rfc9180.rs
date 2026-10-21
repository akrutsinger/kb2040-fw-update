//! RFC 9180 test vector A.3.1: DHKEM(P-256, HKDF-SHA256), HKDF-SHA256, AES-128-GCM, base mode.
//! `p256` does the Diffie-Hellman step that the TPM does on the board.

use hpke::{ENC_LEN, OpenError, open};
use p256::{PublicKey, SecretKey};

const INFO: &str = "4f6465206f6e2061204772656369616e2055726e";
const ENC: &str = "04a92719c6195d5085104f469a8b9814d5838ff72b60501e2c4466e5e67b325ac98536d7b61a1af4b78e5b7f951c0900be863c403ce65c9bfcb9382657222d18c4";
const PK_RM: &str = "04fe8c19ce0905191ebc298a9245792531f26f0cece2460639e8bc39cb7f706a826a779b4cf969b8a0e539c7f62fb3d30ad6aa8f80e30f1d128aafd68a2ce72ea0";
const SK_RM: &str = "f3ce7fdae57e1a310d87f1ebbde6f328be0a99cdbcadf4d6589cf29de4b8ffd2";
// Sequence number 0, the only one a single-shot open uses.
const PT: &str = "4265617574792069732074727574682c20747275746820626561757479";
const AAD: &str = "436f756e742d30";
const CT: &str =
    "5ad590bb8baa577f8619db35a36311226a896e7342a6d836d8b7bcd2f20b6c7f9076ac232e3ab2523f39513434";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// What ECDH_ZGen returns: the x-coordinate of skR * enc.
fn dh() -> [u8; 32] {
    let sk = SecretKey::from_slice(&hex(SK_RM)).unwrap();
    let enc = PublicKey::from_sec1_bytes(&hex(ENC)).unwrap();
    let shared = p256::ecdh::diffie_hellman(sk.to_nonzero_scalar(), enc.as_affine());
    (*shared.raw_secret_bytes()).into()
}

fn open_with(info: &[u8], aad: &[u8], mut message: Vec<u8>) -> Result<Vec<u8>, OpenError> {
    let enc: [u8; ENC_LEN] = hex(ENC).try_into().unwrap();
    let pk_r: [u8; ENC_LEN] = hex(PK_RM).try_into().unwrap();
    open(&dh(), &enc, &pk_r, info, aad, &mut message).map(|pt| pt.to_vec())
}

#[test]
fn opens_the_rfc_vector() {
    assert_eq!(open_with(&hex(INFO), &hex(AAD), hex(CT)), Ok(hex(PT)));
}

#[test]
fn wrong_info_or_aad_fails() {
    assert_eq!(open_with(b"other info", &hex(AAD), hex(CT)), Err(OpenError));
    assert_eq!(open_with(&hex(INFO), b"Count-1", hex(CT)), Err(OpenError));
}

#[test]
fn tampered_ciphertext_fails() {
    for i in [0, CT.len() / 2 - 1] {
        let mut ct = hex(CT);
        ct[i] ^= 1;
        assert_eq!(
            open_with(&hex(INFO), &hex(AAD), ct),
            Err(OpenError),
            "byte {i}"
        );
    }
}

#[test]
fn shorter_than_a_tag_fails() {
    assert_eq!(
        open_with(&hex(INFO), &hex(AAD), vec![0; 15]),
        Err(OpenError)
    );
}
