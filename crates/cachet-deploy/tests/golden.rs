//! The deploy grammar's wire contracts (docs/testing/golden.md).
//!
//! Two artifacts other things read: the plan's JSON, which a deploy tool
//! consumes, and the manifest template, which is deploy-manifest.json and
//! what the deployer reads. Each is pinned as bytes, because a change to
//! either changes what somebody's deployment does. The refusal wordings
//! are a contract too: an operator reads them out of a CI log, and each
//! must say which name to fix.

use cachet_deploy::env::{DOMAIN, EnvMap};
use cachet_deploy::roster::{self, BindingKind, Requirement, env as deploy};
use cachet_deploy::{DeployError, Manifest, plan};

/// The values that are not plain words, by deploy-time name. Everything
/// else takes a word, because the plan shape-checks only the host, the
/// domain, and the grace window and passes the rest through.
const SHAPED: [(&str, &str); 6] = [
    (deploy::HOST, "cache.acme.example"),
    (deploy::ORGS, "acme, acme-labs"),
    (deploy::ADMINS, "ada,grace"),
    (deploy::GC_GRACE_MS, "0"),
    (deploy::PREVIOUS_PUBLIC_KEYS, "cache.acme.example-1:AAAA"),
    (
        deploy::CLOUDFLARE_ACCOUNT_ID,
        "0123456789abcdef0123456789abcdef",
    ),
];

/// A deployment's environment, built from the roster rather than typed
/// out, so a binding that lands reaches the snapshot without anyone
/// remembering to add it here.
///
/// `optional` decides whether the optional values are set, which is the
/// difference between the fullest deployment and the leanest one.
fn env_from_roster(optional: bool) -> EnvMap {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for binding in roster::BINDINGS {
        let Some(env_name) = binding.env else {
            continue;
        };
        let wanted = match binding.requirement {
            Requirement::Required => binding.default.is_none() || optional,
            Requirement::Optional => optional,
            Requirement::Derived => false,
        };
        if !wanted || matches!(binding.kind, BindingKind::Platform(_)) {
            continue;
        }
        let value = SHAPED
            .iter()
            .find(|(name, _)| *name == env_name)
            .map_or("value", |(_, value)| *value);
        pairs.push((env_name.to_owned(), value.to_owned()));
    }
    if optional {
        pairs.push((DOMAIN.to_owned(), "cache2.acme.example".to_owned()));
    }
    EnvMap::from_pairs(pairs)
}

#[test]
fn the_full_plan_is_the_locked_shape() {
    let built = plan("production", &env_from_roster(true)).expect("a full environment plans");
    insta::assert_json_snapshot!("plan_full", built);
}

#[test]
fn the_minimum_plan_is_the_locked_shape() {
    // Only what bootstrap writes: the four required vars and the two
    // secrets. The defaults fill the rest, and no optional surface is on.
    let built = plan("cachet-staging", &env_from_roster(false)).expect("the minimum plans");
    assert!(
        !built
            .secrets
            .iter()
            .any(|secret| secret == "CACHET_STATS_TOKEN")
    );
    insta::assert_json_snapshot!("plan_minimum", built);
}

#[test]
fn the_manifest_template_is_the_locked_shape() {
    // The version is fixed here so the snapshot does not move with every
    // release; deploy-manifest.json carries the real one and the drift
    // check pins that file against the emitter.
    insta::assert_json_snapshot!("manifest_template", Manifest::template("0.0.0"));
}

#[test]
fn every_refusal_names_a_field_and_nothing_else() {
    let minimum = env_from_roster(false);
    let without = |name: &str| {
        EnvMap::from_pairs(
            minimum
                .names()
                .filter(|present| *present != name)
                .map(|present| (present, minimum.get(present).unwrap_or_default())),
        )
    };
    let with = |name: &str, value: &str| {
        let mut pairs: Vec<(String, String)> = minimum
            .names()
            .map(|present| {
                (
                    present.to_owned(),
                    minimum.get(present).unwrap_or_default().to_owned(),
                )
            })
            .collect();
        pairs.push((name.to_owned(), value.to_owned()));
        EnvMap::from_pairs(pairs)
    };
    let mut wordings = Vec::new();
    for (label, error) in [
        ("bad name", plan("Production", &minimum).unwrap_err()),
        (
            "empty environment",
            plan("production", &EnvMap::default()).unwrap_err(),
        ),
        (
            "one missing",
            plan("production", &without(deploy::ADMINS)).unwrap_err(),
        ),
        (
            "malformed host",
            plan("production", &with(deploy::HOST, "-cache.acme.example")).unwrap_err(),
        ),
        (
            "malformed domain",
            plan("production", &with(DOMAIN, "cache..acme.example")).unwrap_err(),
        ),
        (
            "malformed grace",
            plan("production", &with(deploy::GC_GRACE_MS, "soon")).unwrap_err(),
        ),
        (
            "half a pair",
            plan("production", &with(deploy::STATS_TOKEN, "token")).unwrap_err(),
        ),
    ] {
        wordings.push(format!("{label}: {error}"));
    }
    insta::assert_snapshot!("refusal_wordings", wordings.join("\n"));
}

#[test]
fn a_missing_required_secret_is_named_with_the_vars() {
    // One refusal names the set, so an operator fixes the file once.
    let vars_only = EnvMap::from_pairs([
        (deploy::HOST, "cache.acme.example"),
        (deploy::ORGS, "acme"),
        (deploy::ADMINS, "ada"),
        (deploy::OAUTH_CLIENT_ID, "Iv1.abc"),
    ]);
    assert_eq!(
        plan("production", &vars_only).unwrap_err(),
        DeployError::Missing(vec![deploy::SIGNING_KEY, deploy::OAUTH_CLIENT_SECRET])
    );
}
