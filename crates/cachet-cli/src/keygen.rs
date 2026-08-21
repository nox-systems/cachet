//! The bootstrap keypair: 32 bytes of OS entropy become a nix-format
//! ed25519 pair. The operator runs this once at deploy time; the secret
//! text goes into the worker's signing-key binding and the public text is
//! what every client's nix.conf learns to trust.

use std::fmt;

/// Why a generation attempt failed.
#[derive(Debug)]
pub enum KeygenError {
    /// The OS refused entropy.
    Entropy(getrandom::Error),
    /// The key name cannot name a nix key.
    BadName(String),
}

impl fmt::Display for KeygenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entropy(inner) => write!(f, "the OS gave no entropy: {inner}"),
            Self::BadName(name) => write!(
                f,
                "the key name {name:?} cannot name a nix key: use <host>-1, no colon, no blanks"
            ),
        }
    }
}

impl std::error::Error for KeygenError {}

/// A name names a nix key when it is nonempty and carries neither the
/// field separator nor whitespace, which is also what keeps the narinfo
/// `Sig` line's grammar total.
///
/// # Errors
///
/// [`KeygenError::BadName`] on a name outside that grammar.
pub fn check_name(name: &str) -> Result<(), KeygenError> {
    if name.is_empty()
        || name
            .chars()
            .any(|ch| ch == ':' || ch.is_whitespace() || !ch.is_ascii())
    {
        return Err(KeygenError::BadName(name.to_string()));
    }
    Ok(())
}

/// Generate one pair: the secret document and the public document, both
/// in nix's `<name>:<base64>` form.
///
/// # Errors
///
/// [`KeygenError::Entropy`] when the OS will not give 32 bytes,
/// [`KeygenError::BadName`] on a name outside the grammar.
pub fn generate(name: &str) -> Result<(String, String), KeygenError> {
    check_name(name)?;
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(KeygenError::Entropy)?;
    let key = cachet_crypto::ed25519::NixSecretKey::from_seed(name, &seed)
        .map_err(|_| KeygenError::BadName(name.to_string()))?;
    Ok((key.to_secret_text(), key.public_key_text()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_obey_the_grammar() {
        assert!(check_name("cache.example.com-1").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("has:colon").is_err());
        assert!(check_name("has blank").is_err());
        assert!(check_name("nón-ascii").is_err());
    }

    #[test]
    fn two_generations_differ_and_parse() {
        let (secret_a, public_a) = generate("example-1").expect("first pair");
        let (secret_b, _) = generate("example-1").expect("second pair");
        assert_ne!(secret_a, secret_b, "entropy must not repeat");
        let parsed = cachet_crypto::ed25519::NixSecretKey::parse(&secret_a).expect("parses back");
        assert_eq!(parsed.public_key_text(), public_a);
    }
}
