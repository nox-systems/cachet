//! The write path's guards (CLAUDE.md §4): decisions every write shares
//! before any byte is read, the claim checks a narinfo must survive before
//! its stored NAR is even probed, and the typestate that makes
//! verify-then-sign structural: the signing step accepts only what the
//! verifier constructed. A write without `Content-Length` is refused
//! because the size guard cannot run on an unknown length, and the guard
//! must precede the read or it is not a guard.

use crate::constants::NAR_EXPANSION_RATIO_MAX;
use crate::error::{ClientError, Result};
use crate::narinfo::Narinfo;
use crate::types::StorePathHash;

/// Require a declared body size within a cap.
///
/// # Errors
///
/// - [`ClientError::LengthRequired`] when the header is absent, empty, or
///   not a non-negative integer. 411 rather than 400: the request is
///   well-formed but missing the one thing the size guard needs, and the
///   client fixes it by declaring a length.
/// - [`ClientError::BodyTooLarge`] when the declared size exceeds the cap.
pub fn require_content_length(header: Option<&str>, cap_bytes: u64) -> Result<u64> {
    let Some(declared) = header else {
        return Err(ClientError::LengthRequired);
    };
    if declared.is_empty() || !declared.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::LengthRequired);
    }
    let length = declared
        .parse::<u64>()
        .map_err(|_| ClientError::LengthRequired)?;
    if length > cap_bytes {
        return Err(ClientError::BodyTooLarge);
    }
    Ok(length)
}

/// How far a NAR write's decoder may run before it refuses.
///
/// Two bounds, and the decode stops at the lower one. The client declares
/// what its frame decodes to, which is what an honest write needs so the
/// decoder can refuse a frame that keeps producing bytes. That
/// declaration is attacker-chosen, so on its own a one-kilobyte upload
/// could ask for a sixty-gigabyte decode; the second bound is how far the
/// uploaded bytes may expand, which ties the work to bytes the client
/// actually sent. A compression bomb therefore costs its author
/// proportional upload, and an honest NAR never approaches either bound
/// (ordinary store paths decompress near threefold).
pub fn nar_decode_bound(compressed_bytes: u64, declared_nar_bytes: u64) -> u64 {
    declared_nar_bytes.min(compressed_bytes.saturating_mul(NAR_EXPANSION_RATIO_MAX))
}

/// The claims a narinfo document makes about itself, checked before the
/// bucket is touched. Two of them are easy to overlook. A narinfo is
/// stored under the hash in its request path, and nothing forces the
/// document's own `StorePath` to agree with it; without this check a
/// client could store, under one path's key, metadata describing another.
/// And the suffix is the signing path's promise: the worker verifies only
/// the compressions it can stream through its decoder, so any other one is
/// refused rather than silently trusted.
///
/// # Errors
///
/// [`ClientError::StorePathMismatch`] when the document's hash disagrees
/// with the request key; [`ClientError::UnsupportedCompression`] for a
/// suffix outside `""` and `.zst`.
pub fn check_narinfo_claims(narinfo: &Narinfo, request_hash: &StorePathHash) -> Result<()> {
    if narinfo.store_path_hash != *request_hash {
        return Err(ClientError::StorePathMismatch);
    }
    match narinfo.url.suffix() {
        "" | ".zst" => Ok(()),
        _ => Err(ClientError::UnsupportedCompression),
    }
}

/// A narinfo whose stored NAR verified byte-for-byte. Only this module
/// constructs it, after the checks below pass: the worker's signing step
/// accepts `&VerifiedNar` and nothing else, which is what makes the order
/// verify-then-sign rather than a convention (CLAUDE.md §1).
///
/// FileHash and FileSize are set from the measured facts: where the client
/// declared them, verification proved them equal; where it omitted them,
/// the stored document gains the computed values, so what the cache serves
/// always describes the bytes it holds.
#[derive(Debug, Clone)]
pub struct VerifiedNar {
    narinfo: Narinfo,
}

impl VerifiedNar {
    /// Verify the stored-NAR measurements against the narinfo's claims and
    /// produce the signable document.
    ///
    /// The nar claims run before the file claims: the decompressed side is
    /// what a client verifies after substituting, so its disagreement is
    /// reported first.
    ///
    /// # Errors
    ///
    /// [`ClientError::NarHashMismatch`] when the decompressed size or hash
    /// disagrees with `NarSize` or `NarHash`;
    /// [`ClientError::FileHashMismatch`] when the compressed hash disagrees
    /// with the hash the NAR key names, or with `FileHash` when declared,
    /// or the compressed size disagrees with `FileSize` when declared.
    pub fn verify(
        narinfo: &Narinfo,
        decompressed_size: u64,
        decompressed_hash_text: &str,
        compressed_hash_nix32: &str,
        compressed_size: u64,
    ) -> Result<Self> {
        if decompressed_size != narinfo.nar_size_bytes || decompressed_hash_text != narinfo.nar_hash
        {
            return Err(ClientError::NarHashMismatch);
        }
        if compressed_hash_nix32 != narinfo.url.file_hash() {
            return Err(ClientError::FileHashMismatch);
        }
        if narinfo
            .file_hash
            .as_ref()
            .is_some_and(|hash| *hash != format!("sha256:{compressed_hash_nix32}"))
        {
            return Err(ClientError::FileHashMismatch);
        }
        if narinfo
            .file_size_bytes
            .is_some_and(|size| size != compressed_size)
        {
            return Err(ClientError::FileHashMismatch);
        }
        Ok(Self {
            narinfo: narinfo
                .with_file_info(format!("sha256:{compressed_hash_nix32}"), compressed_size),
        })
    }

    /// The verified document, for canonicalizing and signing.
    pub fn document(&self) -> &Narinfo {
        &self.narinfo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_malformed_lengths_are_411() {
        for header in [
            None,
            Some(""),
            Some("12x3"),
            Some("-5"),
            Some(" 12"),
            Some("3.7"),
        ] {
            assert_eq!(
                require_content_length(header, 100),
                Err(ClientError::LengthRequired),
                "{header:?}"
            );
        }
    }

    #[test]
    fn over_the_cap_is_413() {
        assert_eq!(
            require_content_length(Some("101"), 100),
            Err(ClientError::BodyTooLarge)
        );
        assert_eq!(require_content_length(Some("100"), 100), Ok(100));
    }

    #[test]
    fn an_unparseable_length_is_411() {
        // Digits but beyond u64: the length exists as a concept the server
        // cannot represent exactly, and the client can fix that by
        // declaring a real one.
        let huge = "1".repeat(30);
        assert_eq!(
            require_content_length(Some(&huge), 100),
            Err(ClientError::LengthRequired)
        );
    }

    fn document(url: &str) -> Narinfo {
        Narinfo::parse(&format!(
            "StorePath: /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-pkg\nURL: {url}\nNarHash: sha256:0iqi0\nNarSize: 12\n"
        ))
        .expect("the document parses")
    }

    #[test]
    fn the_store_path_must_name_the_request_key() {
        let doc = document(&format!("nar/{}.nar.zst", "g".repeat(52)));
        let other = StorePathHash::parse(&"b".repeat(32)).expect("valid");
        assert_eq!(
            check_narinfo_claims(&doc, &other),
            Err(ClientError::StorePathMismatch)
        );
        let own = StorePathHash::parse(&"a".repeat(32)).expect("valid");
        assert_eq!(
            check_narinfo_claims(&doc, &own),
            Ok(()),
            "the matching hash passes"
        );
    }

    #[test]
    fn only_uncompressed_or_zstd_verified_compressions_sign() {
        let own = StorePathHash::parse(&"a".repeat(32)).expect("valid");
        for suffix in ["xz", "br", "gz", "bz2", "lzip", "lz4"] {
            let doc = document(&format!("nar/{}.nar.{suffix}", "g".repeat(52)));
            assert_eq!(
                check_narinfo_claims(&doc, &own),
                Err(ClientError::UnsupportedCompression),
                "{suffix}"
            );
        }
        let plain = document(&format!("nar/{}.nar", "g".repeat(52)));
        assert_eq!(check_narinfo_claims(&plain, &own), Ok(()));
    }

    #[test]
    fn verification_rejects_hash_and_size_lies() {
        let doc = document(&format!("nar/{}.nar.zst", "g".repeat(52)));
        // Nar claims run before file claims.
        assert_eq!(
            VerifiedNar::verify(&doc, 13, "sha256:0iqi0", &"g".repeat(52), 42).unwrap_err(),
            ClientError::NarHashMismatch
        );
        assert_eq!(
            VerifiedNar::verify(&doc, 12, "sha256:0iqi1", &"g".repeat(52), 42).unwrap_err(),
            ClientError::NarHashMismatch
        );
        // A compressed hash that is not the key's hash is a file mismatch.
        assert_eq!(
            VerifiedNar::verify(&doc, 12, "sha256:0iqi0", &"w".repeat(52), 42).unwrap_err(),
            ClientError::FileHashMismatch
        );
        // The agreeing frame verifies, and the document gains FileHash and
        // FileSize.
        let verified =
            VerifiedNar::verify(&doc, 12, "sha256:0iqi0", &"g".repeat(52), 42).expect("agrees");
        let document = verified.document();
        assert_eq!(
            document.file_hash.as_deref(),
            Some(format!("sha256:{}", "g".repeat(52)).as_str())
        );
        assert_eq!(document.file_size_bytes, Some(42));
    }

    #[test]
    fn the_decode_bound_takes_the_lower_of_the_two() {
        // An honest NAR: the declaration is far below the ratio bound, so
        // the declaration is what stops the decoder.
        assert_eq!(nar_decode_bound(131_961_582, 409_205_960), 409_205_960);
        // A bomb: a kilobyte declaring sixty-four gigabytes gets the ratio
        // bound instead, which is proportional to what it uploaded.
        assert_eq!(nar_decode_bound(1_024, u64::MAX), 1_024 * 200);
        // The ratio never overflows into a larger bound.
        assert_eq!(nar_decode_bound(u64::MAX, 4_096), 4_096);
    }

    #[test]
    fn declared_file_facts_agree_or_refuse() {
        let mut doc = document(&format!("nar/{}.nar.zst", "g".repeat(52)));
        doc.file_hash = Some(format!("sha256:{}", "g".repeat(52)));
        doc.file_size_bytes = Some(41);
        assert_eq!(
            VerifiedNar::verify(&doc, 12, "sha256:0iqi0", &"g".repeat(52), 42).unwrap_err(),
            ClientError::FileHashMismatch
        );
        doc.file_size_bytes = Some(42);
        doc.file_hash = Some(format!("sha256:{}", "w".repeat(52)));
        assert_eq!(
            VerifiedNar::verify(&doc, 12, "sha256:0iqi0", &"g".repeat(52), 42).unwrap_err(),
            ClientError::FileHashMismatch
        );
        // When both declared frames agree, the document keeps them.
        let mut agreed = document(&format!("nar/{}.nar.zst", "g".repeat(52)));
        agreed.file_hash = Some(format!("sha256:{}", "g".repeat(52)));
        agreed.file_size_bytes = Some(42);
        let verified = VerifiedNar::verify(&agreed, 12, "sha256:0iqi0", &"g".repeat(52), 42)
            .expect("declared and measured frames agree");
        assert_eq!(verified.document().file_size_bytes, Some(42));
    }
}
