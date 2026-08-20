//! ed25519 signing in the nix binary-cache key format
//! (`<name>:<base64>`), the custody-critical half of cachet's security
//! story: the worker signs a narinfo's fingerprint only after the write-path
//! verifier produced it, and the secret itself never leaves this module's
//! zeroizing wrapper. Signatures are deterministic by design, so the
//! fixture signed by real nix must reproduce exactly here, byte for byte,
//! and the golden lane proves it does.

use base64::Engine;
use ed25519_dalek::Signer;
use zeroize::Zeroizing;

/// Why a key blob failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// The text was not `<name>:<base64>`.
    Shape,
    /// The base64 did not decode.
    Base64,
    /// The decoded length was not 64 (secret) or 32 (public) bytes.
    Length,
    /// The secret blob's public half disagrees with its secret half.
    MismatchedHalves,
}

impl core::fmt::Display for KeyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Shape => "a nix key is <name>:<base64>",
            Self::Base64 => "the key blob is not base64",
            Self::Length => "the key blob has the wrong length",
            Self::MismatchedHalves => "the secret blob's halves disagree",
        })
    }
}

impl std::error::Error for KeyError {}

/// A parsed nix secret key: the 64-byte secret blob (seed + public half)
/// plus the key's name. Signing never needs a generator (ed25519 is
/// deterministic), so this type holds bytes, not state.
pub struct NixSecretKey {
    name: String,
    blob: Zeroizing<[u8; 64]>,
}

impl NixSecretKey {
    /// Parse `<name>:<base64(64 bytes)>`.
    ///
    /// # Errors
    ///
    /// [`KeyError`] on shape, base64, or length failures, and on the blob's
    /// public half disagreeing with the seed half (a corrupt handoff).
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        let (name, blob64) = text.trim().split_once(':').ok_or(KeyError::Shape)?;
        if name.is_empty() {
            return Err(KeyError::Shape);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(blob64)
            .map_err(|_| KeyError::Base64)?;
        let arr = <[u8; 64]>::try_from(&bytes[..]).map_err(|_| KeyError::Length)?;
        let blob = Zeroizing::new(arr);
        let signing = ed25519_dalek::SigningKey::from_bytes(
            <&[u8; 32]>::try_from(&blob[..32]).expect("slice of 64"),
        );
        if signing.verifying_key().to_bytes() != blob[32..] {
            return Err(KeyError::MismatchedHalves);
        }
        Ok(Self {
            name: name.to_string(),
            blob,
        })
    }

    /// Derive a key from a 32-byte seed (as `cachet keygen` produces).
    ///
    /// # Errors
    ///
    /// [`KeyError`] only structurally: derivation cannot fail.
    pub fn from_seed(name: &str, seed: &[u8; 32]) -> Result<Self, KeyError> {
        let signing = ed25519_dalek::SigningKey::from_bytes(seed);
        let mut blob = Zeroizing::new([0u8; 64]);
        blob[..32].copy_from_slice(seed);
        blob[32..].copy_from_slice(&signing.verifying_key().to_bytes());
        Ok(Self {
            name: name.to_string(),
            blob,
        })
    }

    /// The key's name, the `Sig` field's prefix.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The public half in nix's `<name>:<base64(32 bytes)>` format.
    pub fn public_key_text(&self) -> String {
        let key = ed25519_dalek::VerifyingKey::from_bytes(
            <&[u8; 32]>::try_from(&self.blob[32..]).expect("slice of 64"),
        )
        .expect("the halves were checked at parse");
        format!(
            "{}:{}",
            self.name,
            base64::engine::general_purpose::STANDARD.encode(key.to_bytes())
        )
    }

    /// Sign a narinfo fingerprint, returning the `Sig` field's value:
    /// `<name>:<base64(64-byte signature)>`. Deterministic: the same
    /// fingerprint and key always produce the same bytes.
    pub fn sign_fingerprint(&self, fingerprint: &str) -> String {
        let signing = ed25519_dalek::SigningKey::from_bytes(
            <&[u8; 32]>::try_from(&self.blob[..32]).expect("slice of 64"),
        );
        let signature = signing.sign(fingerprint.as_bytes());
        format!(
            "{}:{}",
            self.name,
            base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
        )
    }
}

impl core::fmt::Debug for NixSecretKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // why: secrets never Debug-print their bytes; the name is the only
        // permissible surface.
        write!(
            f,
            "NixSecretKey {{ name: {:?}, blob: <redacted> }}",
            self.name
        )
    }
}

/// Verify a `Sig` value against a fingerprint and a public key blob (32
/// bytes, the public half's base64-decoded form). Used by tests and the
/// doctor probe; the worker never verifies before it signs.
pub fn verify_fingerprint(fingerprint: &str, sig_value: &str, public_key: &[u8; 32]) -> bool {
    let Some((name, sig64)) = sig_value.split_once(':') else {
        return false;
    };
    let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(sig64) else {
        return false;
    };
    let Ok(sig) = ed25519_dalek::Signature::try_from(&sig_bytes[..]) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let _ = name;
    ed25519_dalek::Verifier::verify(&key, fingerprint.as_bytes(), &sig).is_ok()
}

/// Parse a nix public key line `<name>:<base64(32 bytes)>`.
///
/// # Errors
///
/// [`KeyError`] on shape, base64, or length failures.
pub fn parse_public_key(text: &str) -> Result<(String, [u8; 32]), KeyError> {
    let (name, blob64) = text.trim().split_once(':').ok_or(KeyError::Shape)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(blob64)
        .map_err(|_| KeyError::Base64)?;
    <[u8; 32]>::try_from(&bytes[..])
        .map(|arr| (name.to_string(), arr))
        .map_err(|_| KeyError::Length)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = include_str!("../../../fixtures/nix-signed/secret");
    const PUBLIC: &str = include_str!("../../../fixtures/nix-signed/public");
    const NARINFO: &str =
        include_str!("../../../fixtures/nix-signed/qvqa04f0m85m0a6xxnan5vxnwg2jkgl9.narinfo");

    #[test]
    fn the_fixture_round_trips() {
        let key = NixSecretKey::parse(SECRET).expect("the fixture parses");
        assert_eq!(key.name(), "cachet-fixture-1");
        assert_eq!(key.public_key_text(), PUBLIC.trim());
    }

    /// The signature must equal real nix's bytes exactly: ed25519 is
    /// deterministic and our fingerprint recipe is the nix one, so any
    /// divergence here breaks every client verify in the integration lane.
    #[test]
    fn our_signatures_are_nix_signatures() {
        let key = NixSecretKey::parse(SECRET).expect("the fixture parses");
        let document = cachet_core::narinfo::Narinfo::parse(NARINFO).expect("the narinfo parses");
        let sig = key.sign_fingerprint(&document.fingerprint());
        let expected = document
            .signatures
            .iter()
            .find(|s| s.starts_with("cachet-fixture-1:"))
            .expect("the fixture carries our key's signature");
        assert_eq!(&sig, expected);
    }

    #[test]
    fn verification_accepts_and_rejects() {
        let key = NixSecretKey::parse(SECRET).expect("parses");
        let document = cachet_core::narinfo::Narinfo::parse(NARINFO).expect("parses");
        let (_, public) = parse_public_key(PUBLIC).expect("parses");
        let sig = key.sign_fingerprint(&document.fingerprint());
        assert!(verify_fingerprint(&document.fingerprint(), &sig, &public));
        assert!(!verify_fingerprint(
            &format!("{}x", document.fingerprint()),
            &sig,
            &public
        ));
    }

    #[test]
    fn secret_never_debugs_bytes() {
        let key = NixSecretKey::parse(SECRET).expect("parses");
        let shown = format!("{key:?}");
        assert!(!shown.contains("Tv1t"), "the blob never prints: {shown}");
        assert!(shown.contains("cachet-fixture-1"));
    }
}
