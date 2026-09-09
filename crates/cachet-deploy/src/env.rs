//! The environment map: the values the grammar reads, behind a type
//! rather than an ambient `std::env` call.
//!
//! CLAUDE.md §3's seam law says pure code reads no ambient environment,
//! and a deployment plan is exactly the kind of thing that should be
//! reproducible from stated inputs. The caller builds the map; the
//! grammar reads it.

use std::collections::BTreeMap;

/// The custom domain override. Deploy-time only: it names the domain to
/// attach, defaults to the host, and never becomes a binding.
pub const DOMAIN: &str = "CACHET_DEPLOY_DOMAIN";

/// A read-only view of the environment the plan derives from.
///
/// `BTreeMap` rather than a hash: the plan renders in a stable order, and
/// a stable order is what makes a plan diffable between two runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvMap(BTreeMap<String, String>);

impl EnvMap {
    /// Builds a map from pairs.
    ///
    /// Values are trimmed, and a value that is then empty reads as absent.
    /// This is the rule `infra/src/config.ts` applied, and the deploy
    /// workflow depends on it: an optional secret the GitHub environment
    /// does not hold renders as the empty string, which must read as unset
    /// rather than as a value (docs/DEPLOY.md).
    ///
    /// # Examples
    /// ```
    /// use cachet_deploy::env::EnvMap;
    /// use cachet_deploy::roster::env::HOST;
    /// let env = EnvMap::from_pairs([(HOST, " cache.example.com "), ("CACHET_DEPLOY_FONT_CSS", "")]);
    /// assert_eq!(env.get(HOST), Some("cache.example.com"));
    /// assert!(!env.has("CACHET_DEPLOY_FONT_CSS"));
    /// ```
    #[must_use]
    pub fn from_pairs<K, V, I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into().trim().to_owned()))
                .filter(|(_, value): &(String, String)| !value.is_empty())
                .collect(),
        )
    }

    /// Reads one name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Whether a name is present.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// Every name present, in stable order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_or_blank_value_reads_as_absent() {
        let env =
            EnvMap::from_pairs([(DOMAIN, "   "), ("CACHET_DEPLOY_HOST", "cache.example.com")]);
        assert!(!env.has(DOMAIN));
        assert_eq!(env.get("CACHET_DEPLOY_HOST"), Some("cache.example.com"));
    }

    #[test]
    fn the_names_read_in_a_stable_order() {
        let one = EnvMap::from_pairs([("B", "2"), ("A", "1")]);
        let other = EnvMap::from_pairs([("A", "1"), ("B", "2")]);
        assert_eq!(one, other, "insertion order is not part of the map");
        assert_eq!(one.names().collect::<Vec<_>>(), ["A", "B"]);
    }
}
