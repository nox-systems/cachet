//! The two presence filters, decided over probe answers as data: pass one
//! drops paths the upstream substituter already serves, pass two drops
//! paths cachet itself already holds. An answer a client cannot get is
//! `None`, and the path stays: a probe failure must never become a drop,
//! because rebuilding is the safe side of the error.

/// The pass-one tally, in candidate order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterTally {
    /// Paths this pass kept, roots first then survivors in candidate
    /// order, which is the order the lease writes them in.
    pub kept: Vec<String>,
    /// Paths the upstream probe answered present.
    pub upstream_hits: usize,
    /// Probes that produced no answer.
    pub probe_failures: usize,
}

/// The pass-two tally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheTally {
    /// Paths to upload, in candidate order.
    pub to_upload: Vec<String>,
    /// Paths cachet itself already held.
    pub cache_hits: usize,
    /// Probes that produced no answer.
    pub probe_failures: usize,
}

/// Pass one: roots always keep (never probed); every other candidate
/// takes one upstream probe, with `Some(true)` dropping it and `None`
/// keeping it counted.
pub fn filter_against_upstream(
    candidates: &[String],
    root_paths: &std::collections::BTreeSet<String>,
    probe: &dyn Fn(&str) -> Option<bool>,
) -> FilterTally {
    let mut tally = FilterTally {
        kept: Vec::new(),
        upstream_hits: 0,
        probe_failures: 0,
    };
    for path in candidates {
        if root_paths.contains(path) {
            tally.kept.push(path.clone());
            continue;
        }
        match probe(path) {
            Some(true) => tally.upstream_hits += 1,
            Some(false) => tally.kept.push(path.clone()),
            None => {
                tally.probe_failures += 1;
                tally.kept.push(path.clone());
            }
        }
    }
    tally
}

/// Pass two: cachet's own HEAD answers `Some(true)` for held paths; every
/// survivor of pass one is probed, roots included.
pub fn drop_already_cached(
    candidates: &[String],
    probe: &dyn Fn(&str) -> Option<bool>,
) -> CacheTally {
    let mut tally = CacheTally {
        to_upload: Vec::new(),
        cache_hits: 0,
        probe_failures: 0,
    };
    for path in candidates {
        match probe(path) {
            Some(true) => tally.cache_hits += 1,
            Some(false) => tally.to_upload.push(path.clone()),
            None => {
                tally.probe_failures += 1;
                tally.to_upload.push(path.clone());
            }
        }
    }
    tally
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn roots_skip_the_upstream_probe() {
        let roots: std::collections::BTreeSet<String> =
            paths(&["/nix/store/a"]).into_iter().collect();
        let tally =
            filter_against_upstream(&paths(&["/nix/store/a", "/nix/store/b"]), &roots, &|path| {
                assert_ne!(path, "/nix/store/a", "a root must never be probed");
                Some(false)
            });
        assert_eq!(tally.kept, paths(&["/nix/store/a", "/nix/store/b"]));
        assert_eq!(tally.upstream_hits, 0);
        assert_eq!(tally.probe_failures, 0);
    }

    #[test]
    fn presence_drops_absence_keeps_unknown_keeps_counted() {
        let tally = filter_against_upstream(
            &paths(&["/nix/store/hit", "/nix/store/miss", "/nix/store/unk"]),
            &std::collections::BTreeSet::new(),
            &|path| match path {
                "/nix/store/hit" => Some(true),
                "/nix/store/miss" => Some(false),
                _ => None,
            },
        );
        assert_eq!(tally.kept, paths(&["/nix/store/miss", "/nix/store/unk"]));
        assert_eq!(tally.upstream_hits, 1);
        assert_eq!(tally.probe_failures, 1);
    }

    #[test]
    fn the_cache_pass_has_no_roots_exception() {
        let tally = drop_already_cached(&paths(&["/nix/store/x", "/nix/store/y"]), &|path| {
            (path == "/nix/store/x").then_some(true)
        });
        assert_eq!(tally.to_upload, paths(&["/nix/store/y"]));
        assert_eq!(tally.cache_hits, 1);
    }
}
