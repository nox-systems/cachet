//! The upload plan: staging-layout reading, request ordering, per-object
//! mechanics, and URL construction. Every decision here is pure data; the
//! pipeline executes what these functions answer.

use cachet_core::constants::UPLOAD_SINGLE_MAX_BYTES;
use cachet_core::multipart::{PlanShape, plan_shape};

use crate::error::PushError;

/// One object as the staging directory reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedObject {
    /// The bucket key, the request path, and the staging-relative file
    /// path: one string names all three, by nix's construction of the
    /// staging tree.
    pub key: String,
    /// Its size in bytes.
    pub size_bytes: u64,
}

impl StagedObject {
    /// Whether the object rides the narinfo ordering slot.
    pub fn is_narinfo(&self) -> bool {
        self.key
            .ends_with(cachet_core::constants::NARINFO_KEY_SUFFIX)
    }
}

/// Read a staging directory's layout the nix way: top-level `*.narinfo`
/// files, and the object files under `nar/`. Everything else is the
/// pipeline's own noise and fails the read.
///
/// # Errors
///
/// [`PushError::StagingUnreadable`] for an entry that fits neither shape.
pub fn read_staging_layout(entries: &[(String, u64)]) -> Result<Vec<StagedObject>, PushError> {
    let mut objects = Vec::with_capacity(entries.len());
    for (name, size_bytes) in entries {
        // why: `nix copy --to file://` marks its staging root with the
        // cache-info document; it is transport metadata, never an object.
        if name == "nix-cache-info" {
            continue;
        }
        let key =
            if name.ends_with(cachet_core::constants::NARINFO_KEY_SUFFIX) && !name.contains('/') {
                name.clone()
            } else if let Some(rest) = name.strip_prefix("nar/") {
                if rest.contains('/') {
                    return Err(PushError::StagingUnreadable {
                        message: format!("{name} nests deeper than nar/"),
                    });
                }
                name.clone()
            } else {
                return Err(PushError::StagingUnreadable {
                    message: format!("{name} fits neither the narinfo shape nor nar/"),
                });
            };
        objects.push(StagedObject {
            key,
            size_bytes: *size_bytes,
        });
    }
    Ok(objects)
}

/// The upload order: every NAR before every narinfo, stable inside each
/// kind, because the cache refuses a narinfo whose NAR is absent
/// (NEVER-DANGLE).
pub fn upload_order(objects: &mut [StagedObject]) {
    objects.sort_by_key(StagedObject::is_narinfo);
}

/// The keys one survivor set owns: each survivor's own narinfo key plus
/// the NAR key its staged narinfo names. `nix copy` answers closures, so
/// the staging tree holds far more than the survivors; this is where the
/// filter verdicts bind the wire set again. The NAR key is read out of
/// the staged narinfo, never recomputed: two runs of nix's zstd can
/// encode one NAR to different names, and only the pair staged together
/// names itself consistently.
///
/// # Errors
///
/// [`PushError::Detail`] naming the hash when a staged narinfo does not
/// parse: an unreadable pair never gets a guessed name.
pub fn owned_object_keys(
    survivors: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeSet<String>, PushError> {
    let mut owned = std::collections::BTreeSet::new();
    for (hash, body) in survivors {
        let document =
            cachet_core::narinfo::Narinfo::parse(body).map_err(|_| PushError::Detail {
                message: format!("the staged narinfo for {hash} does not parse"),
            })?;
        owned.insert(format!(
            "{hash}{}",
            cachet_core::constants::NARINFO_KEY_SUFFIX
        ));
        owned.insert(document.url.as_str().to_string());
    }
    Ok(owned)
}

/// One object's ride: the whole body in one PUT, or the multipart
/// quartet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadMechanics {
    /// `PUT /{key}` with the whole body.
    Single,
    /// The multipart plan: part count and final length from the shared
    /// part math.
    Multipart(PlanShape),
}

/// Decide the mechanics. Nix's stat answers the size; an implausible
/// answer fails before any byte moves. The single-PUT cap compares
/// inclusively: exactly-at-cap rides single, as the worker's guard
/// agrees. The multipart refusal delegates to the shared plan's own
/// reasoning.
///
/// # Errors
///
/// [`PushError::TooLarge`] past the parts cap;
/// [`PushError::ImplausibleSize`]!? — sizes come from stat so this branch
/// exists for completeness against adapters, expecting never to fire.
pub fn plan_mechanics(key: &str, size_bytes: u64) -> Result<UploadMechanics, PushError> {
    if size_bytes <= UPLOAD_SINGLE_MAX_BYTES {
        return Ok(UploadMechanics::Single);
    }
    plan_shape(size_bytes)
        .map(UploadMechanics::Multipart)
        .map_err(|_| PushError::TooLarge {
            key: key.to_string(),
        })
}

/// The request URL: trailing slashes stripped from the base once, the
/// key verbatim, the query appended. Project and upload ids are owned by
/// grammar-checked constructors upstream of here.
pub fn object_url(cache_url: &str, key: &str, query: &str) -> String {
    format!("{}/{key}{query}", cache_url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::owned_object_keys;

    const SURVIVOR_HASH: &str = "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
    const NAR_KEY: &str = "nar/nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn.nar.zst";

    fn survivor_body(store_path: &str, nar_key: &str) -> String {
        format!(
            "StorePath: {store_path}\nURL: {nar_key}\nCompression: zstd\nNarHash: sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j\nNarSize: 42\n"
        )
    }

    #[test]
    fn owned_keys_is_exactly_the_survivor_pairs() {
        let survivors = std::collections::BTreeMap::from([(
            SURVIVOR_HASH.to_string(),
            survivor_body(&format!("/nix/store/{SURVIVOR_HASH}-built-1"), NAR_KEY),
        )]);
        let owned = owned_object_keys(&survivors).expect("parses");
        assert_eq!(
            owned,
            std::collections::BTreeSet::from([
                format!("{SURVIVOR_HASH}.narinfo"),
                NAR_KEY.to_string(),
            ]),
        );
    }

    #[test]
    fn a_shared_nar_is_owned_once() {
        let other_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let survivors = std::collections::BTreeMap::from([
            (
                SURVIVOR_HASH.to_string(),
                survivor_body(&format!("/nix/store/{SURVIVOR_HASH}-built-1"), NAR_KEY),
            ),
            (
                other_hash.to_string(),
                survivor_body(&format!("/nix/store/{other_hash}-built-2"), NAR_KEY),
            ),
        ]);
        let owned = owned_object_keys(&survivors).expect("parses");
        assert_eq!(owned.len(), 3, "two narinfos naming one NAR key share it");
    }

    #[test]
    fn an_unparseable_survivor_narinfo_names_its_hash() {
        let survivors = std::collections::BTreeMap::from([(
            SURVIVOR_HASH.to_string(),
            "not a narinfo".to_string(),
        )]);
        let failure = owned_object_keys(&survivors).expect_err("refuses");
        assert!(
            failure.to_string().contains(SURVIVOR_HASH),
            "the error names the hash: {failure}"
        );
    }

    #[test]
    fn the_layout_reads_the_two_shapes() {
        let entries = vec![
            ("nix-cache-info".to_string(), 40_u64),
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narinfo".to_string(),
                1_000_u64,
            ),
            ("nar/xxxx.nar.zst".to_string(), 2_000_u64),
        ];
        let objects = read_staging_layout(&entries).expect("the layout reads");
        assert_eq!(objects.len(), 2, "nix-cache-info is metadata, skipped");
        assert_eq!(objects[0].key, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narinfo");
        assert_eq!(objects[1].key, "nar/xxxx.nar.zst");
        assert!(read_staging_layout(&[("README".to_string(), 1)]).is_err());
        assert!(read_staging_layout(&[("nar/deep/x".to_string(), 1)]).is_err());
    }

    #[test]
    fn narinfos_go_last_in_stable_order() {
        let mut objects = read_staging_layout(&[
            ("c".to_string() + ".narinfo", 1),
            ("nar/b.nar.zst".to_string(), 2),
            ("a".to_string() + ".narinfo", 1),
            ("nar/d.nar.zst".to_string(), 2),
        ])
        .expect("reads");
        upload_order(&mut objects);
        let keys: Vec<&str> = objects.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["nar/b.nar.zst", "nar/d.nar.zst", "c.narinfo", "a.narinfo"]
        );
    }

    #[test]
    fn the_cap_rides_single_and_past_it_goes_multipart() {
        assert_eq!(
            plan_mechanics("nar/x.nar", UPLOAD_SINGLE_MAX_BYTES).expect("at cap"),
            UploadMechanics::Single,
        );
        match plan_mechanics("nar/x.nar", UPLOAD_SINGLE_MAX_BYTES + 1).expect("past cap") {
            UploadMechanics::Multipart(shape) => {
                assert_eq!(
                    shape.count,
                    (UPLOAD_SINGLE_MAX_BYTES + 1)
                        .div_ceil(cachet_core::constants::UPLOAD_PART_BYTES),
                );
                assert!(shape.last_len > 0);
            }
            UploadMechanics::Single => panic!("past the cap must not ride single"),
        }
    }

    #[test]
    fn urls_strip_the_base_once() {
        assert_eq!(
            object_url("https://cachet.example.com/", "nar/x.nar.zst", ""),
            "https://cachet.example.com/nar/x.nar.zst",
        );
        assert_eq!(
            object_url("https://cachet.example.com//", "x.narinfo", "?uploads"),
            "https://cachet.example.com/x.narinfo?uploads",
        );
    }
}
