//! The deploy grammar's pure laws over arbitrary inputs
//! (docs/testing/property.md). The grammar is a function of its two
//! arguments, every derived name hangs off the deployment's name, and no
//! rendered error or plan ever carries a value out of the environment map.

use hegel::TestCase;
use hegel::generators as gs;

use cachet_deploy::env::{DOMAIN, EnvMap};
use cachet_deploy::roster::env as deploy;
use cachet_deploy::{Deployment, plan};

/// The names a plan reads, plus names it ignores, so a sweep reaches both
/// halves of a map.
const NAMES: [&str; 10] = [
    deploy::HOST,
    deploy::ORGS,
    deploy::ADMINS,
    deploy::OAUTH_CLIENT_ID,
    deploy::SIGNING_KEY,
    deploy::OAUTH_CLIENT_SECRET,
    deploy::STATS_TOKEN,
    deploy::GC_GRACE_MS,
    DOMAIN,
    "CACHET_DEPLOY_UNKNOWN",
];

/// A word drawn from an alphabet, so a name is usually valid and sometimes
/// not.
fn draw_word(tc: &TestCase, alphabet: &[u8], max: usize) -> String {
    let bytes = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(max));
    bytes
        .iter()
        .map(|byte| alphabet[usize::from(*byte) % alphabet.len()] as char)
        .collect()
}

#[hegel::test(test_cases = 256)]
fn name_validation_is_total(tc: TestCase) {
    // Any byte string in, a typed answer out, never a panic.
    let bytes = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(60));
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    if let Ok(deployment) = Deployment::new(&raw) {
        // Anything accepted names resources the platform will take: a
        // worker or bucket name under 64 characters of lowercase,
        // digits, and dashes, and a dataset with no dash at all.
        let resource = deployment.resource_name();
        assert!(
            resource.len() <= 63,
            "{resource} is longer than a worker name"
        );
        assert!(
            resource
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "{resource} carries a character a worker name cannot"
        );
        assert!(!deployment.dataset().contains('-'));
        // The prefix appears once, whether the operator wrote it or not.
        assert!(resource.starts_with("cachet-"));
        assert!(!resource.starts_with("cachet-cachet-"));
    }
}

#[hegel::test(test_cases = 256)]
fn no_error_ever_renders_a_value_from_the_environment(tc: TestCase) {
    // The map holds the signing key, and an error that could carry a
    // value would eventually print one into a CI log. Every value here is
    // marked, so a leak is unmistakable.
    let count = tc.draw(gs::integers::<u8>()) % 10;
    let mut marked: Vec<(String, String)> = Vec::new();
    for index in 0..count {
        let slot = usize::from(tc.draw(gs::integers::<u8>())) % NAMES.len();
        let tail = draw_word(&tc, b"abcXYZ019:/._@-", 20);
        marked.push((NAMES[slot].to_owned(), format!("zz{index}zz{tail}")));
    }
    let name = draw_word(&tc, b"abcz019-_. ", 34);

    let env = EnvMap::from_pairs(marked.clone());
    let Err(error) = plan(&name, &env) else {
        return;
    };
    let rendered = error.to_string();
    for (env_name, value) in &marked {
        assert!(
            !rendered.contains(value.as_str()),
            "the error rendered {env_name}'s value: {rendered}"
        );
    }
}

#[hegel::test(test_cases = 256)]
fn a_plan_never_carries_a_secret_value(tc: TestCase) {
    // why the zz fences: a bare prefix with an empty drawn tail is a word
    // like "sec", which the JSON's own "secrets" key contains. The fences
    // make every marked value a string no field name can be.
    let key = format!("zzkey{}zz", draw_word(&tc, b"abcXYZ019+/=", 40));
    let secret = format!("zzsec{}zz", draw_word(&tc, b"abcXYZ019", 20));
    let token = format!("zztok{}zz", draw_word(&tc, b"abcXYZ019", 20));
    let name = format!("a{}", draw_word(&tc, b"abcz019", 12));
    let env = EnvMap::from_pairs([
        (deploy::HOST, "cache.example.com"),
        (deploy::ORGS, "acme"),
        (deploy::ADMINS, "ada"),
        (deploy::OAUTH_CLIENT_ID, "Iv1.abc"),
        (deploy::SIGNING_KEY, key.as_str()),
        (deploy::OAUTH_CLIENT_SECRET, secret.as_str()),
        (deploy::STATS_TOKEN, token.as_str()),
        (
            deploy::CLOUDFLARE_ACCOUNT_ID,
            "0123456789abcdef0123456789abcdef",
        ),
    ]);
    let Ok(built) = plan(&name, &env) else {
        return;
    };
    // The plan names its secrets and carries none of their values, which
    // is what makes `cachet plan` printable into a pull request.
    let json = serde_json::to_string(&built).expect("a plan serializes");
    assert!(json.contains("CACHET_SIGNING_KEY"), "the name is expected");
    for (label, value) in [
        ("the key", &key),
        ("the secret", &secret),
        ("the token", &token),
    ] {
        assert!(!json.contains(value.as_str()), "{label} reached the plan");
    }
}

#[hegel::test(test_cases = 128)]
fn the_grammar_is_a_function_of_its_arguments(tc: TestCase) {
    // Pure means reproducible: the same name and the same map answer the
    // same plan on any machine, which is what lets a plan be reviewed
    // before it runs.
    let name = format!("a{}", draw_word(&tc, b"abcz019-", 12));
    let grace = u64::from(tc.draw(gs::integers::<u32>()));
    let env = EnvMap::from_pairs([
        (deploy::HOST, "cache.example.com".to_owned()),
        (deploy::ORGS, "acme, labs".to_owned()),
        (deploy::ADMINS, "ada".to_owned()),
        (deploy::OAUTH_CLIENT_ID, "Iv1.abc".to_owned()),
        (deploy::SIGNING_KEY, "k".to_owned()),
        (deploy::OAUTH_CLIENT_SECRET, "s".to_owned()),
        (deploy::GC_GRACE_MS, grace.to_string()),
    ]);
    assert_eq!(plan(&name, &env), plan(&name, &env));
}

#[hegel::test(test_cases = 256)]
fn the_name_determines_every_other_name(tc: TestCase) {
    let name = format!("a{}", draw_word(&tc, b"abcz019-", 20));
    let Ok(deployment) = Deployment::new(&name) else {
        return;
    };
    let resource = deployment.resource_name();
    assert_eq!(deployment.worker_name(), resource);
    assert_eq!(deployment.bucket_name(), resource);
    assert_eq!(deployment.kv_title(), resource);
    assert_eq!(deployment.dataset(), resource.replace('-', "_"));
    assert!(deployment.env_file_path().ends_with(&name));
}
