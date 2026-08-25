//! Preparing one store path for upload.
//!
//! The pipeline used to hand every surviving path to `nix copy --to
//! file://`, which serialized and compressed each path's whole closure
//! into a scratch directory and then discarded almost all of it: closure
//! members the cache already held were compressed at full cost and
//! filtered out afterwards. Compression is the expensive half of a push,
//! so paying it for paths nobody asked for is the client's largest cost.
//!
//! This module does the same work one path at a time. nix serializes the
//! path and nothing else, the bytes are compressed and hashed as they
//! stream past, and the result is held wherever it fits. Paths are
//! independent, so the pipeline runs many at once and the compression
//! spreads across cores by itself.

use std::path::PathBuf;

use crate::error::PushError;

/// How large a compressed NAR may get before it stops living in memory.
///
/// Most store paths compress to well under this, so most uploads never
/// touch the disk. The threshold exists for the ones that do not: the
/// pipeline keeps several paths in flight at once, and without a ceiling
/// a window full of large paths would hold all of them resident.
pub const SPILL_THRESHOLD_BYTES: usize = 8 * 1024 * 1024;

/// The zstd level the client compresses at.
///
/// Level 3 is zstd's own default and the level nix uses for its binary
/// caches, so what cachet stores compresses like what every other nix
/// cache stores. Higher levels cost the uploader minutes to save the
/// downloader single-digit percent, which is the wrong trade for a cache
/// written once by CI and read continuously.
pub const COMPRESSION_LEVEL: i32 = 3;

/// What nix knows about a store path before any byte moves.
///
/// `nix path-info --json` answers all of it, so a narinfo can be built
/// without the client ever hashing the uncompressed NAR itself: nix
/// already holds `NarHash` and `NarSize` in its database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathFacts {
    /// The full `/nix/store/<hash>-<name>` text.
    pub store_path: String,
    /// The uncompressed NAR's hash, as `sha256:<nix32>`.
    pub nar_hash: String,
    /// The uncompressed NAR's size.
    pub nar_size_bytes: u64,
    /// The path's references, as store-path basenames.
    pub references: Vec<String>,
    /// The deriver's basename, when the path has one.
    pub deriver: Option<String>,
}

/// Where a compressed NAR waits for its upload.
///
/// Both shapes can be sent more than once without the caller holding a
/// second copy: bytes share one allocation behind a refcount, and a
/// spilled file is re-read per attempt. A retry therefore costs a re-send
/// and never a re-compression.
#[derive(Debug, Clone)]
pub enum NarBody {
    /// Small enough to have stayed in memory.
    Bytes(std::sync::Arc<[u8]>),
    /// Spilled to a scratch file, deleted when the last handle drops.
    File(std::sync::Arc<tempfile::TempPath>),
}

impl NarBody {
    /// The compressed byte count.
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::File(path) => std::fs::metadata(path.as_ref()).map_or(0, |meta| meta.len()),
        }
    }

    /// Whether the body carries nothing. A NAR is never empty, so this
    /// exists for the lint and for callers reasoning about the sink.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The scratch file's path, for the ranged reads a multipart upload
    /// makes.
    pub fn spilled_path(&self) -> Option<PathBuf> {
        match self {
            Self::Bytes(_) => None,
            Self::File(path) => Some(path.to_path_buf()),
        }
    }
}

/// One path, serialized and compressed, with what the compressor
/// measured.
#[derive(Debug, Clone)]
pub struct StagedNar {
    /// What nix said about the path.
    pub facts: PathFacts,
    /// The compressed bytes' hash, bare nix32: the NAR key names it.
    pub file_hash_nix32: String,
    /// The compressed byte count.
    pub file_size_bytes: u64,
    /// The bytes themselves.
    pub body: NarBody,
}

impl StagedNar {
    /// The bucket key this NAR uploads to.
    pub fn nar_key(&self) -> String {
        format!(
            "{}{}.nar.zst",
            cachet_core::constants::NAR_KEY_PREFIX,
            self.file_hash_nix32
        )
    }

    /// The narinfo describing this path, unsigned.
    ///
    /// The document is built rather than copied out of a staging tree, so
    /// nothing a previous push left in the local store rides along with
    /// it. That matters for `Sig` in particular: a path substituted from
    /// this very cache carries the cache's own signature in the local
    /// store, and copying it forward made the worker append a second
    /// identical one on every re-push.
    ///
    /// # Errors
    ///
    /// [`PushError::Detail`] when the facts do not form a narinfo the
    /// protocol's own parser accepts, which means nix answered something
    /// this client does not understand.
    pub fn narinfo(&self) -> Result<cachet_core::narinfo::Narinfo, PushError> {
        use std::fmt::Write as _;
        let mut text = String::with_capacity(1024);
        let _ = writeln!(text, "StorePath: {}", self.facts.store_path);
        let _ = writeln!(text, "URL: {}", self.nar_key());
        let _ = writeln!(text, "Compression: zstd");
        let _ = writeln!(text, "FileHash: sha256:{}", self.file_hash_nix32);
        let _ = writeln!(text, "FileSize: {}", self.file_size_bytes);
        let _ = writeln!(text, "NarHash: {}", self.facts.nar_hash);
        let _ = writeln!(text, "NarSize: {}", self.facts.nar_size_bytes);
        let _ = writeln!(text, "References: {}", self.facts.references.join(" "));
        if let Some(deriver) = &self.facts.deriver {
            let _ = writeln!(text, "Deriver: {deriver}");
        }
        cachet_core::narinfo::Narinfo::parse(&text).map_err(|code| PushError::Detail {
            message: format!(
                "the narinfo built for {} is not one this protocol accepts: {}",
                self.facts.store_path,
                code.code()
            ),
        })
    }
}

/// nix answers `NarHash` in SRI form (`sha256-<base64>`); a narinfo spells
/// the same hash `sha256:<nix32>`. Both name the same 32 bytes, so the
/// conversion decodes one and re-encodes the other rather than trusting
/// either spelling.
///
/// # Errors
///
/// [`PushError::Detail`] when the text is neither spelling, or does not
/// decode to a 32-byte digest.
pub fn nar_hash_from_nix(text: &str) -> Result<String, PushError> {
    let unknown = || PushError::Detail {
        message: format!("nix answered a NarHash this client cannot read: {text}"),
    };
    // Already in the narinfo's own spelling: nothing to convert.
    if let Some(rest) = text.strip_prefix("sha256:") {
        if cachet_crypto::base32::decode(rest).is_ok() {
            return Ok(text.to_string());
        }
        return Err(unknown());
    }
    let blob = text.strip_prefix("sha256-").ok_or_else(unknown)?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, blob)
        .map_err(|_| unknown())?;
    if bytes.len() != 32 {
        return Err(unknown());
    }
    Ok(format!("sha256:{}", cachet_crypto::base32::encode(&bytes)))
}

/// One entry of `nix path-info --json`. nix answers a map from store path
/// to either the record or null, and null means the path is not in this
/// store: something removed it between the diff and this call, which is
/// nothing to push.
#[derive(Debug, serde::Deserialize)]
struct NixPathInfo {
    #[serde(rename = "narHash")]
    nar_hash: String,
    #[serde(rename = "narSize")]
    nar_size: u64,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    deriver: Option<String>,
}

/// Read `nix path-info --json` into the facts a narinfo needs.
///
/// Paths nix answered null for are dropped rather than refused, because a
/// path that left the store is not something this push can send and not
/// something that should end it.
///
/// # Errors
///
/// [`PushError::Detail`] when the text is not the shape nix documents, or
/// when a `NarHash` is spelled in a way this client cannot read.
pub fn parse_path_facts(text: &str) -> Result<Vec<PathFacts>, PushError> {
    let answered: std::collections::BTreeMap<String, Option<NixPathInfo>> =
        serde_json::from_str(text).map_err(|failure| PushError::Detail {
            message: format!("nix path-info --json did not parse: {failure}"),
        })?;
    let mut facts = Vec::with_capacity(answered.len());
    for (store_path, info) in answered {
        let Some(info) = info else { continue };
        facts.push(PathFacts {
            nar_hash: nar_hash_from_nix(&info.nar_hash)?,
            nar_size_bytes: info.nar_size,
            references: info
                .references
                .iter()
                .map(|reference| basename(reference).to_string())
                .collect(),
            deriver: info
                .deriver
                .as_deref()
                .map(|deriver| basename(deriver).to_string()),
            store_path,
        });
    }
    Ok(facts)
}

/// A store path's basename, which is how a narinfo spells references and
/// the deriver.
pub fn basename(store_path: &str) -> &str {
    store_path.rsplit('/').next().unwrap_or(store_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> PathFacts {
        PathFacts {
            store_path: format!("/nix/store/{}-pkg", "a".repeat(32)),
            nar_hash: format!("sha256:{}", "0".repeat(52)),
            nar_size_bytes: 222_232,
            references: vec![format!("{}-pkg", "a".repeat(32))],
            deriver: Some(format!("{}-pkg.drv", "b".repeat(32))),
        }
    }

    fn staged() -> StagedNar {
        StagedNar {
            facts: facts(),
            file_hash_nix32: "g".repeat(52),
            file_size_bytes: 4_096,
            body: NarBody::Bytes(std::sync::Arc::from(&b"body"[..])),
        }
    }

    #[test]
    fn the_nar_key_names_the_compressed_hash() {
        assert_eq!(
            staged().nar_key(),
            format!("nar/{}.nar.zst", "g".repeat(52))
        );
    }

    #[test]
    fn the_built_narinfo_carries_no_signature() {
        let document = staged().narinfo().expect("the facts form a narinfo");
        assert!(
            document.signatures.is_empty(),
            "a built document has nothing to inherit"
        );
        assert_eq!(document.nar_size_bytes, 222_232);
        assert_eq!(document.file_size_bytes, Some(4_096));
        assert_eq!(document.compression, "zstd");
        assert_eq!(
            document.extra_lines,
            vec![format!("Deriver: {}-pkg.drv", "b".repeat(32))]
        );
    }

    #[test]
    fn a_built_narinfo_round_trips_through_the_protocol_parser() {
        let document = staged().narinfo().expect("valid");
        let reparsed =
            cachet_core::narinfo::Narinfo::parse(&document.serialize()).expect("its own form");
        assert_eq!(reparsed, document);
    }

    #[test]
    fn nix_sri_hashes_convert_to_the_narinfo_spelling() {
        // The 32 zero bytes, spelled both ways.
        let sri = format!(
            "sha256-{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0_u8; 32])
        );
        let converted = nar_hash_from_nix(&sri).expect("SRI converts");
        assert_eq!(
            converted,
            format!("sha256:{}", cachet_crypto::base32::encode(&[0_u8; 32]))
        );
        // The narinfo spelling passes through untouched.
        assert_eq!(nar_hash_from_nix(&converted).expect("passes"), converted);
    }

    #[test]
    fn unreadable_hashes_refuse_rather_than_guess() {
        for text in ["", "sha512-abc", "sha256-!!!", "sha256:!!!", "abcdef"] {
            assert!(nar_hash_from_nix(text).is_err(), "{text}");
        }
        // Right spelling, wrong digest length.
        let short = format!(
            "sha256-{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0_u8; 16])
        );
        assert!(nar_hash_from_nix(&short).is_err());
    }

    #[test]
    fn nix_path_info_becomes_narinfo_facts() {
        let text = r#"{
          "/nix/store/0nzagg1ly2wxr9d8yqh00gqh7r6m0pkm-libuv-1.52.1": {
            "ca": null,
            "deriver": "/nix/store/ljj249hs9dnzwhrd1xy3bsgdzzkf5q7d-libuv-1.52.1.drv",
            "narHash": "sha256-drTkg2NAolQOTJYcDc4ivJ/dNimkifqgV+LDTusN/gY=",
            "narSize": 222232,
            "references": ["/nix/store/0nzagg1ly2wxr9d8yqh00gqh7r6m0pkm-libuv-1.52.1"],
            "registrationTime": 1786010127,
            "signatures": ["cache.nixos.org-1:2GRXg6WPDd7sdmEn"],
            "ultimate": false
          }
        }"#;
        let facts = parse_path_facts(text).expect("nix's own shape parses");
        assert_eq!(facts.len(), 1);
        let one = &facts[0];
        assert_eq!(one.nar_size_bytes, 222_232);
        assert!(one.nar_hash.starts_with("sha256:"));
        assert_eq!(
            one.references,
            vec!["0nzagg1ly2wxr9d8yqh00gqh7r6m0pkm-libuv-1.52.1"],
            "references are basenames, the way a narinfo spells them"
        );
        assert_eq!(
            one.deriver.as_deref(),
            Some("ljj249hs9dnzwhrd1xy3bsgdzzkf5q7d-libuv-1.52.1.drv")
        );
    }

    #[test]
    fn a_path_nix_no_longer_holds_is_dropped_not_refused() {
        let text = r#"{"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-gone": null}"#;
        assert!(
            parse_path_facts(text)
                .expect("null is an answer")
                .is_empty(),
            "a path that left the store is nothing to push"
        );
    }

    #[test]
    fn an_unreadable_answer_refuses() {
        assert!(parse_path_facts("not json").is_err());
    }

    #[test]
    fn basenames_are_what_a_narinfo_spells() {
        assert_eq!(basename("/nix/store/aaa-pkg"), "aaa-pkg");
        assert_eq!(basename("aaa-pkg"), "aaa-pkg");
    }
}
