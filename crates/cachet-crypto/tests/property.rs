//! The crypto crate's pure laws (docs/testing/property.md): encoding
//! round-trips over arbitrary bytes, streaming equivalence, tamper
//! rejection, and decoder totality.

use cachet_crypto::base32::{DecodeError, decode, encode};
use cachet_crypto::ed25519::{NixSecretKey, parse_public_key, verify_fingerprint};
use cachet_crypto::sha256::{Sha256Stream, sha256};
use cachet_crypto::zstd_stream::decode_all;
use hegel::TestCase;
use hegel::generators as gs;

#[hegel::test(test_cases = 256)]
fn base32_round_trips_for_any_length(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(64));
    let encoded = encode(&bytes);
    assert!(
        encoded
            .bytes()
            .all(|b| b"0123456789abcdfghijklmnpqrsvwxyz".contains(&b))
    );
    let decoded = decode(&encoded).expect("the own encoding decodes");
    assert_eq!(&decoded[..], &bytes[..], "round trip is the identity");
}

#[hegel::test(test_cases = 128)]
fn streaming_hash_equals_one_shot(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(512));
    let cut = tc.draw(gs::integers::<usize>().max_value(bytes.len()));
    let mut stream = Sha256Stream::new();
    stream.update(&bytes[..cut]);
    stream.update(&bytes[cut..]);
    assert_eq!(stream.digest_so_far(), sha256(&bytes));
    assert_eq!(stream.byte_count(), bytes.len() as u64);
}

#[hegel::test(test_cases = 128)]
fn sign_verify_tamper_is_rejected(tc: TestCase) {
    let mut seed = [0u8; 32];
    for chunk in seed.chunks_mut(8) {
        chunk.copy_from_slice(&tc.draw(gs::integers::<u64>()).to_le_bytes());
    }
    let fingerprint: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(128));
    let fingerprint = String::from_utf8_lossy(&fingerprint).into_owned();
    let key = NixSecretKey::from_seed("prop", &seed).expect("seeds derive");
    let sig = key.sign_fingerprint(&fingerprint);
    let (_, public) = parse_public_key(&key.public_key_text()).expect("own public parses");
    assert!(verify_fingerprint(&fingerprint, &sig, &public));

    let mut tampered = fingerprint.as_bytes().to_vec();
    if tampered.is_empty() {
        tampered.push(0);
    }
    let i = tc.draw(gs::integers::<usize>().max_value(tampered.len() - 1));
    tampered[i] ^= 1 << (tc.draw(gs::integers::<u8>()) % 8);
    let tampered = String::from_utf8_lossy(&tampered).into_owned();
    assert!(!verify_fingerprint(&tampered, &sig, &public));
}

// why: the decoder faces uploaded bytes; arbitrary inputs must decode or
// refuse, never panic (CLAUDE.md §7), and the bomb limit must bind.
#[hegel::test(test_cases = 256)]
fn decode_is_total_and_limited(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(256));
    let _ = decode_all(&bytes, 1 << 20);
}

// why: decode faces attacker-chosen strings (narinfo hash fields); any
// input must answer Ok or a typed InvalidDigit, never a panic, and decode
// must be a projection: an Ok answer survives encode-then-decode exactly.
#[hegel::test(test_cases = 256)]
fn base32_decode_is_total_and_a_projection(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(80));
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match decode(&text) {
        Ok(decoded) => {
            assert_eq!(decode(&encode(&decoded)), Ok(decoded));
        }
        Err(DecodeError::InvalidDigit(_)) => {}
    }
}
