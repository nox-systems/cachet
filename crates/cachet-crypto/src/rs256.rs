//! RS256 verification and JWKS parsing for GitHub OIDC (CLAUDE.md §5).
//! `jose` has no Rust equivalent that builds for wasm32 without ring, so
//! verification is pure-rust `rsa` over parsed JWKs, with the claim policy
//! living in cachet-core: this module only proves (or refutes) that bytes
//! signed with a private key belong to a presented key.

use base64ct::{Base64UrlUnpadded, Encoding};

/// Why a JWT or JWKS input failed structurally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtError {
    /// Not three base64url segments.
    Shape,
    /// A segment was not base64url.
    Encoding,
    /// The header or claims were not JSON.
    Json,
    /// The JWK is not an RSA signing key.
    NotRsaSigningKey,
    /// Key material would not construct.
    KeyMaterial,
    /// The signature bytes were not a valid RS256 signature.
    Signature,
}

impl core::fmt::Display for JwtError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Shape => "a JWT has three base64url parts",
            Self::Encoding => "a JWT part is not base64url",
            Self::Json => "a JWT segment is not JSON",
            Self::NotRsaSigningKey => "a JWK is not an RSA signing key",
            Self::KeyMaterial => "the JWK integers do not form a key",
            Self::Signature => "the signature is not well-formed",
        })
    }
}

impl std::error::Error for JwtError {}

/// The pieces of a decoded JWT: header and claims JSON plus the signed
/// input and signature. Verification decisions (algorithm pinning,
/// audience, claims) live in cachet-core; this cut keeps the crypto
/// module honest about what it proved.
#[derive(Debug, Clone)]
pub struct DecodedJwt {
    /// The JOSE header (algorithm, key id), parsed JSON.
    pub header: serde_json::Value,
    /// The claims, parsed JSON.
    pub claims: serde_json::Value,
    /// `base64url(header).base64url(claims)`: exactly what RS256 signs.
    pub signing_input: String,
    /// The raw signature bytes.
    pub signature: Vec<u8>,
    /// The `kid` header field when present.
    pub kid: Option<String>,
}

/// Split a JWT into its verifiable pieces without trusting any field.
///
/// # Errors
///
/// [`JwtError`] on shape, encoding, or JSON failures.
pub fn decode_jwt(token: &str) -> Result<DecodedJwt, JwtError> {
    let mut parts = token.split('.');
    let (Some(h), Some(c), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Shape);
    };
    let header_bytes = Base64UrlUnpadded::decode_vec(h).map_err(|_| JwtError::Encoding)?;
    let claims_bytes = Base64UrlUnpadded::decode_vec(c).map_err(|_| JwtError::Encoding)?;
    let signature = Base64UrlUnpadded::decode_vec(s).map_err(|_| JwtError::Encoding)?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_bytes).map_err(|_| JwtError::Json)?;
    let claims: serde_json::Value =
        serde_json::from_slice(&claims_bytes).map_err(|_| JwtError::Json)?;
    let kid = header
        .get("kid")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Ok(DecodedJwt {
        header,
        claims,
        signing_input: format!("{h}.{c}"),
        signature,
        kid,
    })
}

/// A single RSA JWK from a JWKS document.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RsaJwk {
    /// The key id.
    pub kid: String,
    /// The key type; must be `"RSA"`.
    pub kty: String,
    /// The modulus, base64url big-endian.
    pub n: String,
    /// The public exponent, base64url big-endian.
    pub e: String,
}

/// Verify an RS256 signature over `signing_input` under this JWK.
///
/// # Errors
///
/// [`JwtError`] when the key or signature does not construct; a signature
/// that verifies-false is `Ok(false)`, so callers distinguish broken
/// crypto plumbing from a dishonest token.
pub fn verify_rs256(jwk: &RsaJwk, signing_input: &str, signature: &[u8]) -> Result<bool, JwtError> {
    if jwk.kty != "RSA" {
        return Err(JwtError::NotRsaSigningKey);
    }
    let n_bytes = Base64UrlUnpadded::decode_vec(&jwk.n).map_err(|_| JwtError::Encoding)?;
    let e_bytes = Base64UrlUnpadded::decode_vec(&jwk.e).map_err(|_| JwtError::Encoding)?;
    let key = rsa::RsaPublicKey::new(
        rsa::BigUint::from_bytes_be(&n_bytes),
        rsa::BigUint::from_bytes_be(&e_bytes),
    )
    .map_err(|_| JwtError::KeyMaterial)?;
    let scheme = rsa::Pkcs1v15Sign::new::<sha2::Sha256>();
    Ok(key
        .verify(scheme, signing_input.as_bytes(), signature)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_requires_three_valid_parts() {
        // The signature segment is base64url-canonical (zero trailing
        // bits), like every honest producer's.
        let valid = "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3QifQ.eyJzdWIiOiJib2IifQ.eA";
        let decoded = decode_jwt(valid).expect("well-formed input decodes");
        assert_eq!(decoded.kid.as_deref(), Some("test"));
        assert_eq!(
            decoded.signing_input,
            "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3QifQ.eyJzdWIiOiJib2IifQ"
        );
        assert!(decode_jwt("a.b").is_err());
        assert!(decode_jwt("a.b.c.d").is_err());
        assert!(decode_jwt("a.b.*").is_err());
    }
}
