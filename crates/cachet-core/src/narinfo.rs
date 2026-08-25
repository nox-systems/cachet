//! The narinfo document: parsing, serializing, canonicalization, and the
//! reference graph.
//!
//! A narinfo is a flat list of `Key: value` lines describing one store
//! path. cachet reads it for the `URL` field (which names the NAR that must
//! exist before the document is stored) and the `References` field (the
//! graph the collector walks). Unknown fields are preserved verbatim: we
//! are a cache, not an authority on the format, and a field we do not
//! understand must survive a store-and-serve cycle untouched, because
//! clients verify signatures over content we would otherwise have altered.
//!
//! The parser is total: arbitrary bytes yield a typed failure, never a
//! panic (CLAUDE.md §7).

use crate::constants::{
    DEFAULT_COMPRESSION, NARINFO_BYTES_MAX, NARINFO_LINES_MAX, NARINFO_REFERENCES_MAX,
};
use crate::error::{ClientError, Result};
use crate::keys::{NarKey, parse_store_path, parse_store_path_basename};
use crate::types::StorePathHash;

/// A parsed narinfo: the fields cachet reads, plus everything it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Narinfo {
    /// The full `/nix/store/<hash>-<name>` text of the `StorePath` field.
    pub store_path: String,
    /// The hash half of the store path, computed once at parse time.
    pub store_path_hash: StorePathHash,
    /// The NAR's bucket key: the object the write path checks for.
    pub url: NarKey,
    /// The `Compression` field, defaulting to the nix default when absent.
    pub compression: String,
    /// Hash of the compressed NAR. Absent for some uncompressed layouts.
    pub file_hash: Option<String>,
    /// Size of the compressed NAR.
    pub file_size_bytes: Option<u64>,
    /// Hash of the uncompressed NAR: what `nix store verify` checks.
    pub nar_hash: String,
    /// Size of the uncompressed NAR.
    pub nar_size_bytes: u64,
    /// The `References` field, in arrival order. Canonicalization and the
    /// signature fingerprint use the sorted, deduplicated form instead.
    pub references: Vec<String>,
    /// The `Sig` lines, in arrival order. Signature lines may repeat.
    pub signatures: Vec<String>,
    /// Every line whose key we do not recognise, verbatim and in order.
    pub extra_lines: Vec<String>,
}

impl Narinfo {
    /// Parse a narinfo document.
    ///
    /// # Errors
    ///
    /// Every rejection is a typed [`ClientError`]: `body_too_large` over the
    /// byte cap, `malformed_narinfo` for every grammar failure.
    pub fn parse(text: &str) -> Result<Self> {
        // Bound before splitting, so a huge document costs a length
        // comparison.
        if u64::try_from(text.len()).expect("len fits") > NARINFO_BYTES_MAX {
            return Err(ClientError::BodyTooLarge);
        }
        let collected = collect_fields(text)?;

        let store_path_text = require(&collected.single, "StorePath")?;
        let url_text = require(&collected.single, "URL")?;
        let nar_hash_text = require(&collected.single, "NarHash")?;
        let nar_size_text = require(&collected.single, "NarSize")?;

        let path_parts =
            parse_store_path(&store_path_text).map_err(|_| ClientError::MalformedNarinfo)?;
        let url =
            crate::keys::parse_nar_key(&url_text).map_err(|_| ClientError::MalformedNarinfo)?;
        let nar_hash = check_hash_token(&nar_hash_text)?;
        let nar_size_bytes = parse_byte_count(&nar_size_text)?;
        let references = parse_references(collected.single.get("References"))?;

        let file_hash = match collected.single.get("FileHash") {
            Some(value) => Some(check_hash_token(value)?),
            None => None,
        };
        let file_size_bytes = match collected.single.get("FileSize") {
            Some(value) => Some(parse_byte_count(value)?),
            None => None,
        };

        Ok(Self {
            store_path: store_path_text,
            store_path_hash: path_parts.hash,
            url,
            compression: collected
                .single
                .get("Compression")
                .cloned()
                .unwrap_or_else(|| DEFAULT_COMPRESSION.to_string()),
            file_hash,
            file_size_bytes,
            nar_hash,
            nar_size_bytes,
            references,
            signatures: collected.signatures,
            extra_lines: collected.extra_lines,
        })
    }

    /// Serialize the document in its canonical order: known fields first,
    /// sorted references, then the signatures in arrival order, then the
    /// unknown lines verbatim. `parse(serialize(x))` reproduces `x`.
    #[must_use]
    pub fn serialize(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(1024);
        let _ = writeln!(out, "StorePath: {}", self.store_path);
        let _ = writeln!(out, "URL: {}", self.url.as_str());
        let _ = writeln!(out, "Compression: {}", self.compression);
        if let Some(hash) = &self.file_hash {
            let _ = writeln!(out, "FileHash: {hash}");
        }
        if let Some(size) = self.file_size_bytes {
            let _ = writeln!(out, "FileSize: {size}");
        }
        let _ = writeln!(out, "NarHash: {}", self.nar_hash);
        let _ = writeln!(out, "NarSize: {}", self.nar_size_bytes);
        // Canonical ordering of references matches the set nix builds when
        // it parses, so served bytes and the client-side fingerprint agree.
        let _ = writeln!(out, "References: {}", self.canonical_references().join(" "));
        for signature in &self.signatures {
            let _ = writeln!(out, "Sig: {signature}");
        }
        for line in &self.extra_lines {
            let _ = writeln!(out, "{line}");
        }
        out
    }

    /// The references as nix would materialize them: sorted and deduplicated
    /// basenames. A client fingerprint rebuilds its reference set from the
    /// document, so signing must speak in exactly this order.
    pub fn canonical_references(&self) -> Vec<String> {
        let mut refs = self.references.clone();
        refs.sort();
        refs.dedup();
        refs
    }

    /// The fingerprint's reference set: every basename with the store
    /// directory restored. The narinfo document prints basenames, but the
    /// fingerprint nix computes (ValidPathInfo::fingerprint, path-info.cc)
    /// passes the set through `StoreDirConfig::printStorePathSet`, which
    /// prepends the store directory to each one: signing basenames is the
    /// reason nix refused every cachet signature until this fix.
    fn fingerprint_references(&self) -> Vec<String> {
        self.canonical_references()
            .iter()
            .map(|reference| format!("/nix/store/{reference}"))
            .collect()
    }

    /// The store-path hashes this narinfo depends on, for the closure walk.
    /// Parsing already validated every reference, so re-parsing cannot fail.
    pub fn reference_hashes(&self) -> Vec<StorePathHash> {
        self.references
            .iter()
            .map(|reference| {
                parse_store_path_basename(reference)
                    .expect("references are validated at parse")
                    .hash
            })
            .collect()
    }

    /// The nix signature fingerprint: `1;<storePath>;<narHash>;<narSize>;`
    /// followed by the sorted, deduplicated references joined by commas.
    /// Signed directly with ed25519; there is no outer hash.
    pub fn fingerprint(&self) -> String {
        format!(
            "1;{};{};{};{}",
            self.store_path,
            self.nar_hash,
            self.nar_size_bytes,
            self.fingerprint_references().join(",")
        )
    }

    /// With the measured compressed-NAR facts filled in. The signing
    /// pipeline computes them from the bytes it verified, so the served
    /// document states what the cache actually holds.
    #[must_use]
    pub fn with_file_info(&self, file_hash: String, file_size_bytes: u64) -> Self {
        Self {
            file_hash: Some(file_hash),
            file_size_bytes: Some(file_size_bytes),
            ..self.clone()
        }
    }

    /// With one additional signature line appended, preserving the order of
    /// any pre-existing lines.
    #[must_use]
    pub fn with_signature(&self, signature: String) -> Self {
        let mut signatures = self.signatures.clone();
        // why: appending unconditionally made a re-push grow the document.
        // A path substituted from this cache carries the cache's own Sig
        // in the local store, so a client that copied its narinfo forward
        // handed back a document already signed, and every cycle appended
        // another identical line until the document hit its byte cap and
        // pushes started failing. A signature that is already present says
        // exactly what a second copy would.
        if !signatures.contains(&signature) {
            signatures.push(signature);
        }
        Self {
            signatures,
            ..self.clone()
        }
    }
}

/// Fields collected during a parse, before validation.
struct FieldBag {
    single: std::collections::BTreeMap<String, String>,
    signatures: Vec<String>,
    extra_lines: Vec<String>,
}

/// The fields a document may carry once each.
const SINGLE_VALUE_FIELDS: [&str; 8] = [
    "StorePath",
    "URL",
    "Compression",
    "FileHash",
    "FileSize",
    "NarHash",
    "NarSize",
    "References",
];

/// Split the document into fields, keeping unknown lines aside.
fn collect_fields(text: &str) -> Result<FieldBag> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() > NARINFO_LINES_MAX {
        return Err(ClientError::MalformedNarinfo);
    }
    let mut single = std::collections::BTreeMap::new();
    let mut signatures = Vec::new();
    let mut extra_lines = Vec::new();

    for line in lines {
        // A trailing newline yields one empty final element; blank lines
        // are otherwise not meaningful in this format, so they are dropped
        // rather than preserved.
        if line.is_empty() {
            continue;
        }
        let Some(separator) = line.find(": ") else {
            return Err(ClientError::MalformedNarinfo);
        };
        if separator == 0 {
            return Err(ClientError::MalformedNarinfo);
        }
        let key = &line[..separator];
        let value = &line[separator + 2..];

        if key == "Sig" {
            signatures.push(value.to_string());
            continue;
        }
        if !SINGLE_VALUE_FIELDS.contains(&key) {
            extra_lines.push(line.to_string());
            continue;
        }
        // A duplicated single-valued field is a document we cannot interpret
        // without guessing which one is authoritative, so it is refused
        // rather than resolved.
        if single.insert(key.to_string(), value.to_string()).is_some() {
            return Err(ClientError::MalformedNarinfo);
        }
    }
    Ok(FieldBag {
        single,
        signatures,
        extra_lines,
    })
}

/// Require one of the four fields a narinfo cannot live without.
fn require(single: &std::collections::BTreeMap<String, String>, field: &str) -> Result<String> {
    single
        .get(field)
        .cloned()
        .ok_or(ClientError::MalformedNarinfo)
}

/// A hash field: opaque to us, but it must be one non-empty token.
fn check_hash_token(text: &str) -> Result<String> {
    if text.is_empty() || text.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(ClientError::MalformedNarinfo);
    }
    Ok(text.to_string())
}

/// A non-negative integer, strictly: no signs, no whitespace, no exponents.
fn parse_byte_count(text: &str) -> Result<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ClientError::MalformedNarinfo);
    }
    text.parse::<u64>()
        .map_err(|_| ClientError::MalformedNarinfo)
}

/// Validate the `References` field into checked basenames. The cap bounds
/// both this parse and the closure walk's fan-out from one node.
fn parse_references(raw: Option<&String>) -> Result<Vec<String>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let entries: Vec<&str> = raw.split(' ').filter(|entry| !entry.is_empty()).collect();
    if entries.len() > NARINFO_REFERENCES_MAX {
        return Err(ClientError::MalformedNarinfo);
    }
    for entry in &entries {
        parse_store_path_basename(entry).map_err(|_| ClientError::MalformedNarinfo)?;
    }
    Ok(entries.iter().map(|s| (*s).to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Narinfo {
        Narinfo::parse(&format!(
            "StorePath: /nix/store/{}-pkg\nURL: nar/{}.nar.zst\nNarHash: sha256:0iqi0\nNarSize: 12\n",
            "a".repeat(32),
            "g".repeat(52)
        ))
        .expect("the document parses")
    }

    #[test]
    fn signing_twice_leaves_one_signature() {
        let signature = "cachet.example-1:abcd".to_string();
        let once = document().with_signature(signature.clone());
        assert_eq!(once.signatures, vec![signature.clone()]);
        // The re-push case: a client hands back a document this cache
        // already signed, and signing it again must not grow it.
        let twice = once.with_signature(signature.clone());
        assert_eq!(
            twice.signatures,
            vec![signature],
            "a signature already present is not appended again"
        );
        assert_eq!(
            twice.serialize().matches("Sig:").count(),
            1,
            "the served document carries one Sig line"
        );
    }

    #[test]
    fn a_second_distinct_signature_still_joins() {
        let first = document().with_signature("cachet.example-1:aaaa".to_string());
        let rotated = first.with_signature("cachet.example-2:bbbb".to_string());
        assert_eq!(
            rotated.signatures.len(),
            2,
            "a rotated key adds a signature rather than replacing one"
        );
    }
}
