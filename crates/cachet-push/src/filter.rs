//! The presence filter: candidates the cache already holds drop out of
//! the upload set. The probe is one bulk request upstream of here, so
//! this module answers over its result — a set of held store-path
//! hashes — plus the path grammar: a candidate that does not parse stays
//! in the upload set, because rebuilding is the safe side of the error.

/// The probe-filter tally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheTally {
    /// Paths to upload, in candidate order.
    pub to_upload: Vec<String>,
    /// Paths cachet itself already held.
    pub cache_hits: usize,
    /// Candidates that did not parse as store paths; they stay, fail-
    /// toward-rebuild.
    pub unparseable_paths: usize,
}

/// Drop every candidate whose store-path hash the probe answered as
/// held; everything else stays, in candidate order.
pub fn drop_already_cached(
    candidates: &[String],
    present: &std::collections::BTreeSet<String>,
) -> CacheTally {
    let mut tally = CacheTally {
        to_upload: Vec::new(),
        cache_hits: 0,
        unparseable_paths: 0,
    };
    for path in candidates {
        match cachet_core::keys::parse_store_path(path) {
            Ok(parts) if present.contains(parts.hash.as_str()) => tally.cache_hits += 1,
            Ok(_) => tally.to_upload.push(path.clone()),
            Err(_) => {
                tally.unparseable_paths += 1;
                tally.to_upload.push(path.clone());
            }
        }
    }
    tally
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(hash_letter: char, name: &str) -> String {
        format!("/nix/store/{}-{name}", hash_letter.to_string().repeat(32))
    }

    fn hash(hash_letter: char) -> String {
        hash_letter.to_string().repeat(32)
    }

    #[test]
    fn presence_drops_absence_keeps_unparseable_keeps_counted() {
        let held: std::collections::BTreeSet<String> = [hash('a')].into_iter().collect();
        let candidates = vec![
            path('a', "hit"),
            path('b', "miss"),
            "not a store path".to_string(),
        ];
        let tally = drop_already_cached(&candidates, &held);
        assert_eq!(
            tally.to_upload,
            vec![path('b', "miss"), "not a store path".to_string()]
        );
        assert_eq!(tally.cache_hits, 1);
        assert_eq!(tally.unparseable_paths, 1);
    }

    #[test]
    fn an_empty_present_set_uploads_everything_parseable() {
        let candidates = vec![path('a', "one"), path('b', "two")];
        let tally = drop_already_cached(&candidates, &std::collections::BTreeSet::new());
        assert_eq!(tally.to_upload, candidates);
        assert_eq!(tally.cache_hits, 0);
        assert_eq!(tally.unparseable_paths, 0);
    }
}
