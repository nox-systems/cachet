//! The core's pure laws over arbitrary inputs (docs/testing/property.md).
//! Law classes: totality (a function answers Ok or a typed refusal over any
//! input), round-trips, and plan laws for uploads and GC.

use cachet_core::constants::{MULTIPART_PARTS_MAX, UPLOAD_PART_BYTES};
use cachet_core::error::ClientError;
use cachet_core::keys::{
    parse_nar_request_path, parse_narinfo_request_path, parse_store_path, parse_store_path_basename,
};
use cachet_core::multipart::{part_plan, plan_shape};
use cachet_core::narinfo::Narinfo;
use hegel::TestCase;
use hegel::generators as gs;

#[hegel::test(test_cases = 256)]
fn plan_shape_sums_and_bounds(tc: TestCase) {
    let total = tc.draw(gs::integers::<u64>());
    match plan_shape(total) {
        Ok(shape) => {
            assert!(total > 0 && total <= UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX);
            assert_eq!(
                (shape.count - 1) * UPLOAD_PART_BYTES + shape.last_len,
                total
            );
            assert!(shape.count >= 1 && shape.count <= MULTIPART_PARTS_MAX);
            assert!(shape.last_len >= 1 && shape.last_len <= UPLOAD_PART_BYTES);
        }
        Err(code) => {
            assert!(total == 0 || total > UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX);
            assert!(matches!(
                code,
                ClientError::LengthRequired | ClientError::BodyTooLarge
            ));
        }
    }
}

#[hegel::test(test_cases = 256)]
fn part_plan_sums_and_bounds(tc: TestCase) {
    let total = tc.draw(gs::integers::<u64>());
    match part_plan(total) {
        Ok(plan) => {
            assert!(total > 0 && total <= UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX);
            assert_eq!(plan.parts.iter().map(|p| p.len).sum::<u64>(), total);
            assert!(u64::try_from(plan.parts.len()).expect("len fits") <= MULTIPART_PARTS_MAX);
            assert_eq!(plan.parts[0].number, 1);
            let last = plan.parts.len() - 1;
            for (i, part) in plan.parts.iter().enumerate() {
                assert!(part.len > 0);
                assert_eq!(part.number, u64::try_from(i).expect("fits") + 1);
                if i != last {
                    assert_eq!(part.len, UPLOAD_PART_BYTES);
                }
            }
        }
        Err(code) => {
            assert!(total == 0 || total > UPLOAD_PART_BYTES * MULTIPART_PARTS_MAX);
            assert!(matches!(
                code,
                ClientError::LengthRequired | ClientError::BodyTooLarge
            ));
        }
    }
}

// why: the grammars are the path-traversal guard; a panic anywhere in them
// is a client-induced 500. Arbitrary bytes, arbitrary lengths.
#[hegel::test(test_cases = 512)]
fn key_parsers_are_total(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(300));
    let text = String::from_utf8_lossy(&bytes);
    let _ = parse_narinfo_request_path(&text);
    let _ = parse_nar_request_path(&text);
    let _ = parse_store_path(&text);
    let _ = parse_store_path_basename(&text);
}

// why: the narinfo parser faces untrusted bytes on every write; it answers
// Ok or a typed failure over any input, and its work stays linear.
#[hegel::test(test_cases = 512)]
fn narinfo_parse_is_total(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(2_000));
    let text = String::from_utf8_lossy(&bytes);
    let _ = Narinfo::parse(&text);
}

const NIX32: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
// why: store path names allow letters, digits, and `+-._?=` including the
// vowels the base32 alphabet excludes; the two grammars are different.
const NAME_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-._?=";

fn draw_letter(tc: &TestCase, alphabet: &[u8]) -> char {
    let pick = tc.draw(gs::integers::<u8>());
    alphabet[usize::from(pick) % alphabet.len()] as char
}

fn draw_nix32(tc: &TestCase, len: usize) -> String {
    (0..len).map(|_| draw_letter(tc, NIX32)).collect()
}

fn draw_name(tc: &TestCase, len: usize) -> String {
    (0..len.max(1))
        .map(|_| draw_letter(tc, NAME_CHARS))
        .collect()
}

// why: the canonical form is what nix verifies. Serialize then parse then
// serialize is the identity on the canonical view of any well-formed
// document, which is what makes the signing pipeline safe to canonicalize.
#[hegel::test(test_cases = 256)]
fn narinfo_canonicalization_is_a_fixed_point(tc: TestCase) {
    let store_hash = draw_nix32(&tc, 32);
    let file_hash = draw_nix32(&tc, 52);
    let name = draw_name(&tc, 12);
    let ref_count = tc.draw(gs::integers::<usize>().max_value(4));
    let refs: Vec<String> = (0..ref_count)
        .map(|_| format!("{}-{}", draw_nix32(&tc, 32), draw_name(&tc, 6)))
        .collect();
    // Deliberately scrambled field order: canonicalization must settle it.
    let body = format!(
        "References: {}\nNarSize: 123\nURL: nar/{file_hash}.nar.zst\nStorePath: /nix/store/{store_hash}-{name}\nX-Unknown: preserved verbatim\nSig: cache:nhQ==\nCompression: zstd\nNarHash: sha256:{}\n",
        refs.join(" "),
        draw_nix32(&tc, 52)
    );
    let parsed = Narinfo::parse(&body).expect("generated documents parse");
    let once = parsed.serialize();
    let twice = Narinfo::parse(&once)
        .expect("canonical form reparses")
        .serialize();
    assert_eq!(once, twice, "canonicalization is a fixed point");

    // Field order in the canonical form is exactly the locked order.
    let order: Vec<&str> = once
        .lines()
        .map(|line| line.split(':').next().expect("a key"))
        .collect();
    let expected = [
        "StorePath",
        "URL",
        "Compression",
        "NarHash",
        "NarSize",
        "References",
        "Sig",
        "X-Unknown",
    ];
    assert_eq!(order, expected);
}

// why: the fingerprint is the signature input nix reproduces; drift in it
// fails `nix store verify` for every artifact cachet signs.
#[hegel::test(test_cases = 128)]
fn fingerprint_matches_its_recipe(tc: TestCase) {
    let store_hash = draw_nix32(&tc, 32);
    let name = draw_name(&tc, 12);
    let nar_hash = draw_nix32(&tc, 52);
    let body = format!(
        "StorePath: /nix/store/{store_hash}-{name}\nURL: nar/{}.nar\nNarHash: sha256:{nar_hash}\nNarSize: 1\n",
        draw_nix32(&tc, 52)
    );
    let document = Narinfo::parse(&body).expect("generated documents parse");
    assert_eq!(
        document.fingerprint(),
        format!("1;/nix/store/{store_hash}-{name};sha256:{nar_hash};1;")
    );
}

// why: leases are the collector's only state; the lenient parser must
// still refuse the two fields the collector's safety depends on, over
// arbitrary JSON.
#[hegel::test(test_cases = 256)]
fn lease_parse_is_total(tc: TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(600));
    let text = String::from_utf8_lossy(&bytes);
    let _ = cachet_core::lease::LeaseDocument::parse(&text);
}

// why: the collector's destructive decisions have a small finite decision
// space per item (reserved, narinfo-shaped, marked, old relative to the
// grace boundary) plus a gate over two counts, so the laws are verified by
// exhausting that space, every case computed independently of the planner
// and compared whole. This is where these laws live: the verifier's cost
// for heap-mutating code under nondeterminism is symbolic heap shape, not
// the law, so the kani lane covers only allocation-free cores
// (docs/testing/kani.md).
#[hegel::test(test_cases = 1)]
fn gc_laws_hold_over_the_exhausted_decision_space(_tc: TestCase) {
    use cachet_core::constants::{SWEEP_MAX_FRACTION_DENOMINATOR, SWEEP_MAX_FRACTION_NUMERATOR};
    use cachet_core::gc::{GateTrip, InventoryItem, plan_deletions};
    use cachet_core::keys::is_reserved_key;
    use cachet_core::types::{StorePathHash, UnixMillis};
    use std::collections::{BTreeMap, BTreeSet};

    const NOW: u64 = 10_000_000_000;
    const GRACE: u64 = 1_000;
    // Age classes: ancient, exactly at the grace boundary (kept), and one
    // millisecond past it (swept). Path 4 is always ancient.
    const AGES: [u64; 3] = [0, NOW - GRACE, NOW - GRACE - 1];

    let hash = |letter: char| StorePathHash::parse(&letter.to_string().repeat(32)).expect("valid");
    let key = |letter: char| format!("{}.narinfo", letter.to_string().repeat(32));
    // Path 3 shares path 1's NAR name on purpose: the shared-NAR survival
    // law needs an owner that is sometimes marked while 3 is swept.
    let url_of = |letter: char| {
        let owner = if letter == '3' { '1' } else { letter };
        cachet_core::keys::parse_nar_key(&format!("nar/{}.nar.zst", owner.to_string().repeat(52)))
            .expect("valid nar key")
    };
    let item = |key: String, uploaded_at_ms: u64| InventoryItem {
        key,
        size_bytes: 10,
        uploaded_at_ms,
    };

    for mark_a in [false, true] {
        for mark_b in [false, true] {
            for age_a in AGES {
                for age_b in AGES {
                    for age_c in AGES {
                        let inventory = vec![
                            item(key('1'), age_a),
                            item(key('2'), age_b),
                            item(key('3'), age_c),
                            item(key('4'), 0),
                            item("roots/some-project".to_string(), NOW),
                            item("uploads/some-id".to_string(), 0),
                        ];
                        let mut marked = BTreeSet::new();
                        let mut marked_urls = BTreeMap::new();
                        if mark_a {
                            marked.insert(hash('1'));
                            marked_urls.insert(hash('1'), url_of('1'));
                        }
                        if mark_b {
                            marked.insert(hash('2'));
                            marked_urls.insert(hash('2'), url_of('2'));
                        }
                        let mut expected_narinfos = Vec::new();
                        let mut candidate_urls = Vec::new();
                        for (letter, age, is_marked) in [
                            ('1', age_a, mark_a),
                            ('2', age_b, mark_b),
                            ('3', age_c, false),
                            ('4', 0, false),
                        ] {
                            if !is_marked && age != NOW - GRACE {
                                expected_narinfos.push(key(letter));
                                candidate_urls.push((hash(letter), url_of(letter)));
                            }
                        }
                        let live_urls: Vec<String> = marked_urls
                            .values()
                            .map(|url| url.as_str().to_string())
                            .collect();
                        let mut expected_nars: Vec<String> = candidate_urls
                            .iter()
                            .filter(|(_, url)| !live_urls.contains(&url.as_str().to_string()))
                            .map(|(_, url)| url.as_str().to_string())
                            .collect();
                        let plan = plan_deletions(
                            &inventory,
                            &marked,
                            &marked_urls,
                            &candidate_urls,
                            UnixMillis::new(NOW),
                            GRACE,
                        );
                        let gate = expected_narinfos.len() * SWEEP_MAX_FRACTION_DENOMINATOR
                            > 4 * SWEEP_MAX_FRACTION_NUMERATOR;
                        if gate {
                            assert!(matches!(
                                plan.gate,
                                Some(GateTrip::SweepFractionExceeded { .. })
                            ));
                            assert!(plan.narinfo_deletes.is_empty());
                            assert!(plan.nar_deletes.is_empty());
                        } else {
                            expected_nars.sort();
                            expected_nars.dedup();
                            assert_eq!(plan.gate, None);
                            assert_eq!(plan.narinfo_deletes, expected_narinfos);
                            assert_eq!(plan.nar_deletes, expected_nars);
                        }
                        for deleted in plan.narinfo_deletes.iter().chain(&plan.nar_deletes) {
                            assert!(!is_reserved_key(deleted));
                        }
                    }
                }
            }
        }
    }
}
