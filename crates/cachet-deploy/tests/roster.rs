//! The binding bijection: the roster and the worker's reads are held to
//! each other.
//!
//! The binding names are one list that lived in several places, and a
//! name added to one and missed in another produces a worker that deploys
//! and then refuses at the first request that reads it. This test reads
//! `cachet-worker`'s source and `cachet-core/src/constants.rs` from disk,
//! the way cachet-api's test reads the committed OpenAPI document, and
//! asserts the two sets agree in both directions: a name read and never
//! rostered fails, and a name rostered and never read fails.
//!
//! It lives here rather than in the worker because the worker is wasm32
//! only and no lane runs its tests on a host (CLAUDE.md §4).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cachet_deploy::roster::{self, BindingKind, names};

/// The repository root, from this crate's own manifest directory.
fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Every `.rs` file under a directory, recursively. Panics when the
/// directory is absent: a gate that scanned nothing would pass while
/// holding nothing (CLAUDE.md §9).
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("{} does not list: {err}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry reads").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The binding-shaped string literals in one source text: every quoted
/// `CACHET_*` or `CLOUDFLARE_*` word, plus the two platform names that
/// carry no prefix.
fn literals(source: &str, into: &mut BTreeSet<String>) {
    // why the naive split: these files quote no escaped double quotes, so
    // odd segments are the literals. A stricter tokenizer would be a
    // parser for a problem a split solves.
    for (index, segment) in source.split('"').enumerate() {
        if index % 2 == 0 {
            continue;
        }
        let shaped = segment.len() > 2
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if !shaped {
            continue;
        }
        let namespaced = segment.starts_with("CACHET_")
            || segment.starts_with("CLOUDFLARE_")
            || segment == names::CACHE_BUCKET
            || segment == names::ASSETS;
        if namespaced {
            into.insert(segment.to_owned());
        }
    }
}

#[test]
fn every_binding_the_worker_reads_is_rostered_and_every_rostered_binding_is_read() {
    let mut files = Vec::new();
    sources(&root().join("crates/cachet-worker/src"), &mut files);
    files.push(root().join("crates/cachet-core/src/constants.rs"));
    assert!(files.len() > 10, "the worker's source tree is where it was");

    let mut read = BTreeSet::new();
    for file in &files {
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|err| panic!("{} does not read: {err}", file.display()));
        literals(&source, &mut read);
    }

    // What the worker reads through a roster constant rather than a
    // literal, so the scan above cannot see it.
    let named: BTreeSet<String> = [names::CACHE_BUCKET, names::CACHET_KV, names::ASSETS]
        .into_iter()
        .map(str::to_owned)
        .collect();

    // A compile-time stamp, read with option_env! and never a binding.
    let stamps = ["CACHET_BUILD_SHA"];

    for name in read.iter().filter(|name| !stamps.contains(&name.as_str())) {
        assert!(
            roster::binding(name).is_some(),
            "{name} is read by the worker and missing from the roster"
        );
    }
    for binding in roster::BINDINGS {
        let seen = read.contains(binding.name) || named.contains(binding.name);
        assert!(
            seen,
            "{} is rostered and never read by the worker ({:?})",
            binding.name, binding.kind
        );
    }
    // The platform bindings reach the worker through env.bucket, env.kv,
    // env.analytics_engine, and env.assets; every one is accounted for
    // above, so this is the count that would change if one were dropped.
    assert_eq!(roster::platform().count(), 4);
    assert!(roster::BINDINGS.iter().all(|binding| {
        !matches!(binding.kind, BindingKind::Platform(_)) || binding.env.is_none()
    }));
}
