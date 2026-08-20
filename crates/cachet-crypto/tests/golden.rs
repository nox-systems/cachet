//! The crypto crate's golden vectors (docs/testing/golden.md): everything
//! here was produced by real nix tooling at fixture time, and the lane
//! proves our computations reproduce it byte for byte (the strongest
//! canonical-form evidence this repo carries: the fixture narinfo was
//! emitted by `nix copy` itself).

use base64ct::Encoding as _;
use cachet_crypto::base32;
use cachet_crypto::ed25519::{NixSecretKey, parse_public_key, verify_fingerprint};
use cachet_crypto::sha256::sha256;
use cachet_crypto::zstd_stream::decode_all;

const NARINFO: &str =
    include_str!("../../../fixtures/nix-signed/qvqa04f0m85m0a6xxnan5vxnwg2jkgl9.narinfo");
const NAR_ZST: &[u8] = include_bytes!(
    "../../../fixtures/nix-signed/11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y.nar.zst"
);
const SECRET: &str = include_str!("../../../fixtures/nix-signed/secret");
const PUBLIC: &str = include_str!("../../../fixtures/nix-signed/public");
const NIX_CACHE_INFO: &str = include_str!("../../../fixtures/nix-signed/nix-cache-info");

// Real nix emitted this narinfo for exactly this NAR: our parse of it must
// state the same facts the fields carry.
#[test]
fn fixture_narinfo_facts() {
    let document = cachet_core::narinfo::Narinfo::parse(NARINFO).expect("the fixture parses");
    let nar = decode_all(NAR_ZST, 1_000_000).expect("the fixture NAR decodes");
    assert_eq!(
        document.fingerprint(),
        format!(
            "1;/nix/store/qvqa04f0m85m0a6xxnan5vxnwg2jkgl9-payload;sha256:{};{};",
            base32::encode(&sha256(&nar)),
            nar.len()
        ),
        "the fingerprint over measured bytes agrees with nix's signed one"
    );
    // And the signature line nix emitted covers that fingerprint.
    let key = NixSecretKey::parse(SECRET).expect("the fixture key parses");
    let (_, public) = parse_public_key(PUBLIC).expect("the public half parses");
    let signed = key.sign_fingerprint(&document.fingerprint());
    assert!(
        verify_fingerprint(&document.fingerprint(), &signed, &public),
        "our signed bytes verify under the fixture public key"
    );
    assert!(
        document.signatures.contains(&signed),
        "our signature IS the fixture's signature, byte for byte: {signed}"
    );
}

// Our canonical serialization of nix's own emission reproduces it exactly:
// field order, unknown-field preservation, trailing whitespace on the
// empty References line, the final newline.
#[test]
fn canonical_form_is_byte_exact_against_nix() {
    let document = cachet_core::narinfo::Narinfo::parse(NARINFO).expect("the fixture parses");
    assert_eq!(document.serialize(), NARINFO);
}

// The compression suffix mapping: the file hash of the stored object IS
// the hash in its key name, over the compressed bytes.
#[test]
fn file_hash_names_the_compressed_object() {
    assert_eq!(
        base32::encode(&sha256(NAR_ZST)),
        "11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y"
    );
    assert_eq!(NIX_CACHE_INFO, "StoreDir: /nix/store\n");
}

// why: RS256's verify contract (the rsa crate takes the message DIGEST)
// is invisible to any test that verifies our own output: a signature must
// come from out of house. Node's crypto signed this fixed message under
// the fixture keypair; the lane holds both directions: the vector verifies
// true, and one flipped byte verifies false.
#[test]
fn rs256_verifies_a_node_signed_vector() {
    let jwk: cachet_crypto::rs256::RsaJwk =
        serde_json::from_str(include_str!("../../../fixtures/rs256/public.jwk.json"))
            .expect("the fixture jwk parses");
    let jwk = cachet_crypto::rs256::RsaJwk {
        kid: "golden".to_string(),
        ..jwk
    };
    let message = "the cachet RS256 golden message: 2026-08-20";
    let mut signature = base64ct::Base64UrlUnpadded::decode_vec(
        include_str!("../../../fixtures/rs256/signature.b64u").trim_end(),
    )
    .expect("the fixture signature decodes");
    assert_eq!(
        cachet_crypto::rs256::verify_rs256(&jwk, message, &signature),
        Ok(true)
    );
    signature[0] ^= 1;
    assert_eq!(
        cachet_crypto::rs256::verify_rs256(&jwk, message, &signature),
        Ok(false)
    );
}
