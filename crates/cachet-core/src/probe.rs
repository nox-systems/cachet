//! How the presence probe asks the bucket about the hashes a push names
//! (ADR 0024). The worker runs the plan; this module decides it, so the
//! choice and the keys it reads are a total function a native test can
//! hold to its bounds.
//!
//! Every answer still comes from the bucket itself. A plan only decides
//! which reads ask it: one `head` per narinfo key when the question is
//! small, and a listing of the narinfos that share a first character with
//! some asked hash when it is large.

use std::collections::BTreeSet;

use crate::constants::NARINFO_KEY_SUFFIX;
use crate::types::StorePathHash;

/// The most hashes a probe answers with one bucket head apiece.
///
/// why: a head costs the same whatever the bucket holds, and a listing
/// costs a page per thousand narinfos the listed prefixes hold. On 18
/// October 2026 production held 17,398 narinfos, and a probe that listed
/// all of them took 10 to 13 seconds for two hashes, about two thirds of
/// a second a page. A whole authorized narinfo HEAD from a laptop,
/// credential check and round trip included, took 0.11 to 0.36 seconds,
/// so a bucket head inside the worker costs well under that. The worker
/// keeps six bucket reads waiting at once, the platform's ceiling, so this
/// many heads is 22 rounds of six, a second or two. A larger question
/// lists at most 32 prefixes, however many hashes it names.
pub const PROBE_HEADS_MAX: usize = 128;

/// The reads a probe makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbePlan {
    /// One bucket head for each of these narinfo keys, ascending.
    Heads(Vec<String>),
    /// One listing for each of these key prefixes, ascending: the
    /// distinct first characters of the asked hashes. Every narinfo key
    /// begins with its hash, so an asked hash is held exactly when its
    /// key appears under its own first character.
    Listing(Vec<String>),
}

/// The plan for one probe's validated, deduplicated hashes.
#[must_use]
pub fn plan(asked: &BTreeSet<StorePathHash>) -> ProbePlan {
    if asked.len() <= PROBE_HEADS_MAX {
        return ProbePlan::Heads(
            asked
                .iter()
                .map(|hash| format!("{}{NARINFO_KEY_SUFFIX}", hash.as_str()))
                .collect(),
        );
    }
    let prefixes: BTreeSet<String> = asked
        .iter()
        .filter_map(|hash| hash.as_str().get(..1))
        .map(str::to_string)
        .collect();
    ProbePlan::Listing(prefixes.into_iter().collect())
}

/// The hash a listed key names, when the key is a narinfo's.
#[must_use]
pub fn narinfo_hash(key: &str) -> Option<&str> {
    key.strip_suffix(NARINFO_KEY_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::NIX_BASE32_ALPHABET;

    /// A valid hash that starts with `first` and spells `index` after it.
    fn hash_with(first: char, index: usize) -> StorePathHash {
        let alphabet = NIX_BASE32_ALPHABET.as_bytes();
        let mut text = String::from(first);
        let mut rest = index;
        for _ in 1..32 {
            text.push(char::from(alphabet[rest % alphabet.len()]));
            rest /= alphabet.len();
        }
        StorePathHash::parse(&text).expect("the alphabet makes valid hashes")
    }

    /// `count` distinct valid hashes, spread over the alphabet's first
    /// characters the way real store hashes are.
    fn hashes(count: usize) -> BTreeSet<StorePathHash> {
        let alphabet = NIX_BASE32_ALPHABET.as_bytes();
        (0..count)
            .map(|index| hash_with(char::from(alphabet[index % alphabet.len()]), index))
            .collect()
    }

    #[test]
    fn a_small_question_heads_each_narinfo_and_lists_nothing() {
        let asked = hashes(2);
        let ProbePlan::Heads(keys) = plan(&asked) else {
            panic!("two hashes are under the head bound");
        };
        let expected: Vec<String> = asked
            .iter()
            .map(|hash| format!("{}.narinfo", hash.as_str()))
            .collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn the_bound_is_the_last_size_that_heads() {
        let ProbePlan::Heads(keys) = plan(&hashes(PROBE_HEADS_MAX)) else {
            panic!("the bound itself heads");
        };
        assert_eq!(keys.len(), PROBE_HEADS_MAX);
        assert!(
            matches!(plan(&hashes(PROBE_HEADS_MAX + 1)), ProbePlan::Listing(_)),
            "one past the bound lists",
        );
    }

    #[test]
    fn an_empty_question_reads_nothing() {
        assert_eq!(plan(&BTreeSet::new()), ProbePlan::Heads(Vec::new()));
    }

    #[test]
    fn a_large_question_lists_only_the_first_characters_it_asks_about() {
        // Every hash below starts with 'q' or 'z', so the listing must
        // name those two prefixes and no other.
        let asked: BTreeSet<StorePathHash> = (0..PROBE_HEADS_MAX + 40)
            .map(|index| hash_with(if index % 2 == 0 { 'q' } else { 'z' }, index))
            .collect();
        assert_eq!(asked.len(), PROBE_HEADS_MAX + 40);
        assert_eq!(
            plan(&asked),
            ProbePlan::Listing(vec!["q".to_string(), "z".to_string()]),
        );
    }

    #[test]
    fn every_asked_key_falls_under_a_listed_prefix() {
        let asked = hashes(1_000);
        let ProbePlan::Listing(prefixes) = plan(&asked) else {
            panic!("a thousand hashes list");
        };
        assert!(prefixes.len() <= NIX_BASE32_ALPHABET.len());
        for hash in &asked {
            let key = format!("{}.narinfo", hash.as_str());
            assert!(
                prefixes
                    .iter()
                    .any(|prefix| key.starts_with(prefix.as_str())),
                "{key} is under no listed prefix",
            );
        }
    }

    #[test]
    fn only_a_narinfo_key_names_a_hash() {
        assert_eq!(narinfo_hash("abc.narinfo"), Some("abc"));
        assert_eq!(narinfo_hash("nar/abc.nar.zst"), None);
        assert_eq!(narinfo_hash("roots/org-repo"), None);
    }
}
