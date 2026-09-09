//! The binding roster: every name a cachet deployment's configuration
//! uses, in one place.
//!
//! This table is the source. `tests/roster.rs` holds `cachet-worker` to it
//! by scanning the worker's source and `cachet-core/src/constants.rs`: a
//! name the worker reads and this table lacks fails, and a name this
//! table lists and the worker never reads fails. The deploy-time side is
//! held to it by construction, because the plan and the manifest walk
//! this table rather than a list of their own.
//!
//! # Why the crate has no dependencies by default
//!
//! `cachet-worker` compiles to wasm and imports this module, and a
//! dependency here is a dependency in the bundle. Everything past the
//! roster sits behind the `plan` feature, which the worker leaves off.

/// What a platform binding is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformKind {
    /// An R2 bucket.
    R2Bucket,
    /// A KV namespace.
    KvNamespace,
    /// An Analytics Engine dataset.
    AnalyticsEngine,
    /// The static-asset layer.
    Assets,
}

/// What a binding is, which decides where its value comes from and
/// whether it may ever be printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingKind {
    /// A plain-text var: a host, a comma list, a number. Safe in a
    /// rendered plan, a log, and a pull request.
    Var,
    /// A secret binding. Its value never appears in a plan, a manifest,
    /// or an error, and the property lane asserts that over arbitrary
    /// maps.
    Secret,
    /// A platform resource rather than a value: the bucket, the
    /// namespace, the dataset, the asset layer. The deploy wires it; no
    /// environment variable carries it.
    Platform(PlatformKind),
}

/// Whether a deployment must carry the binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "plan", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "plan", serde(rename_all = "lowercase"))]
pub enum Requirement {
    /// The worker refuses to serve without it. A required var with a
    /// [`Binding::default`] is one the operator may omit, because the
    /// plan supplies the default.
    Required,
    /// The worker runs without it, and omitting it turns one surface off
    /// and breaks nothing else.
    Optional,
    /// The grammar computes it from the deployment's name, so no
    /// environment variable supplies it.
    Derived,
}

/// One binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The name, spelled exactly as every home spells it.
    pub name: &'static str,
    /// Where its value comes from.
    pub kind: BindingKind,
    /// Whether a deployment must carry it.
    pub requirement: Requirement,
    /// The environment variable that supplies it at deploy time. Most
    /// vars keep cachet's `CACHET_DEPLOY_<X>` spelling for the binding
    /// `CACHET_<X>` (ADR 0010). Derived and platform bindings have none,
    /// and so do the overrides only the test lanes set.
    pub env: Option<&'static str>,
    /// The value a required var takes when the environment omits it.
    pub default: Option<&'static str>,
    /// One line an operator reads beside the name.
    pub summary: &'static str,
}

/// The binding names, spelled once. `cachet-worker` imports these rather
/// than repeating the strings.
pub mod names {
    /// The cache's host name.
    pub const CACHET_HOST: &str = "CACHET_HOST";
    /// The GitHub orgs the deployment serves.
    pub const CACHET_ORGS: &str = "CACHET_ORGS";
    /// The GitHub logins with admin rights.
    pub const CACHET_ADMINS: &str = "CACHET_ADMINS";
    /// The GitHub OAuth App's client id.
    pub const CACHET_OAUTH_CLIENT_ID: &str = "CACHET_OAUTH_CLIENT_ID";
    /// The OIDC audience.
    pub const CACHET_AUDIENCE: &str = "CACHET_AUDIENCE";
    /// The ref allowed to renew leases.
    pub const CACHET_DEFAULT_BRANCH_REF: &str = "CACHET_DEFAULT_BRANCH_REF";
    /// The deployment's own name.
    pub const CACHET_DEPLOY_NAME: &str = "CACHET_DEPLOY_NAME";
    /// The collector's cron expression.
    pub const CACHET_GC_CRON: &str = "CACHET_GC_CRON";
    /// The grace window override.
    pub const CACHET_GC_GRACE_MS: &str = "CACHET_GC_GRACE_MS";
    /// The collector's arming switch.
    pub const CACHET_GC_ARMED: &str = "CACHET_GC_ARMED";
    /// The console's licensed-face stylesheet.
    pub const CACHET_FONT_CSS: &str = "CACHET_FONT_CSS";
    /// The Analytics Engine dataset's name.
    pub const CACHET_STATS_DATASET: &str = "CACHET_STATS_DATASET";
    /// The account the counter route queries under.
    pub const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
    /// Where the counter route sends its statement.
    pub const CACHET_STATS_API_URL: &str = "CACHET_STATS_API_URL";
    /// GitHub's JWKS document.
    pub const CACHET_JWKS_URL: &str = "CACHET_JWKS_URL";
    /// The GitHub REST API root.
    pub const CACHET_GITHUB_API_URL: &str = "CACHET_GITHUB_API_URL";
    /// The GitHub web origin.
    pub const CACHET_GITHUB_WEB_URL: &str = "CACHET_GITHUB_WEB_URL";
    /// The signing key.
    pub const CACHET_SIGNING_KEY: &str = "CACHET_SIGNING_KEY";
    /// The GitHub OAuth App's client secret.
    pub const CACHET_OAUTH_CLIENT_SECRET: &str = "CACHET_OAUTH_CLIENT_SECRET";
    /// The Cloudflare token the counter route reads with.
    pub const CACHET_STATS_TOKEN: &str = "CACHET_STATS_TOKEN";
    /// The R2 bucket the whole cache lives in.
    pub const CACHE_BUCKET: &str = "CACHE_BUCKET";
    /// The KV namespace for verdicts, sessions, and OAuth state.
    pub const CACHET_KV: &str = "CACHET_KV";
    /// The dataset every counted thing is written to.
    pub const CACHET_EVENTS: &str = "CACHET_EVENTS";
    /// The asset layer the console serves from.
    pub const ASSETS: &str = "ASSETS";
}

/// The deploy-time spellings of the vars an operator sets (ADR 0010).
pub mod env {
    /// Supplies `CACHET_HOST`.
    pub const HOST: &str = "CACHET_DEPLOY_HOST";
    /// Supplies `CACHET_ORGS`.
    pub const ORGS: &str = "CACHET_DEPLOY_ORGS";
    /// Supplies `CACHET_ADMINS`.
    pub const ADMINS: &str = "CACHET_DEPLOY_ADMINS";
    /// Supplies `CACHET_OAUTH_CLIENT_ID`.
    pub const OAUTH_CLIENT_ID: &str = "CACHET_DEPLOY_OAUTH_CLIENT_ID";
    /// Supplies `CACHET_AUDIENCE`.
    pub const AUDIENCE: &str = "CACHET_DEPLOY_AUDIENCE";
    /// Supplies `CACHET_DEFAULT_BRANCH_REF`.
    pub const DEFAULT_BRANCH_REF: &str = "CACHET_DEPLOY_DEFAULT_BRANCH_REF";
    /// Supplies `CACHET_GC_GRACE_MS`.
    pub const GC_GRACE_MS: &str = "CACHET_DEPLOY_GC_GRACE_MS";
    /// Supplies `CACHET_FONT_CSS`.
    pub const FONT_CSS: &str = "CACHET_DEPLOY_FONT_CSS";
    /// Supplies `CACHET_STATS_TOKEN`. The name changes across the
    /// boundary, and it once did not: the deploy then re-read a variable
    /// nothing had set (`infra/alchemy.run.ts`).
    pub const STATS_TOKEN: &str = "CACHET_DEPLOY_STATS_TOKEN";
    /// Supplies `CLOUDFLARE_ACCOUNT_ID`. Unprefixed on both sides because
    /// every Cloudflare tool in the shell reads this one name.
    pub const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
    /// Supplies `CACHET_SIGNING_KEY`. Unprefixed: it is the worker's
    /// binding name, and bootstrap writes it under that name.
    pub const SIGNING_KEY: &str = "CACHET_SIGNING_KEY";
    /// Supplies `CACHET_OAUTH_CLIENT_SECRET`, unprefixed for the same
    /// reason.
    pub const OAUTH_CLIENT_SECRET: &str = "CACHET_OAUTH_CLIENT_SECRET";
}

/// The whole roster.
///
/// Ordering is deliberate and stable: identity and access first, then the
/// collector, the console, the counters, the overrides only the lanes
/// set, the secrets, and the platform bindings. A rendered plan walks it
/// in this order, so a diff of a plan reads as a diff of intent.
pub const BINDINGS: &[Binding] = &[
    Binding {
        name: names::CACHET_HOST,
        kind: BindingKind::Var,
        requirement: Requirement::Required,
        env: Some(env::HOST),
        default: None,
        summary: "the cache's host name, which is also the signing key's name prefix",
    },
    Binding {
        name: names::CACHET_ORGS,
        kind: BindingKind::Var,
        requirement: Requirement::Required,
        env: Some(env::ORGS),
        default: None,
        summary: "comma-joined GitHub org slugs; nobody outside them authenticates",
    },
    Binding {
        name: names::CACHET_ADMINS,
        kind: BindingKind::Var,
        requirement: Requirement::Required,
        env: Some(env::ADMINS),
        default: None,
        summary: "comma-joined GitHub logins allowed on /api/self/*",
    },
    Binding {
        name: names::CACHET_OAUTH_CLIENT_ID,
        kind: BindingKind::Var,
        requirement: Requirement::Required,
        env: Some(env::OAUTH_CLIENT_ID),
        default: None,
        summary: "the GitHub OAuth App's client id",
    },
    Binding {
        name: names::CACHET_AUDIENCE,
        kind: BindingKind::Var,
        requirement: Requirement::Required,
        env: Some(env::AUDIENCE),
        default: Some("cachet"),
        summary: "the OIDC audience a CI token must carry",
    },
    Binding {
        name: names::CACHET_DEFAULT_BRANCH_REF,
        kind: BindingKind::Var,
        requirement: Requirement::Required,
        env: Some(env::DEFAULT_BRANCH_REF),
        default: Some("refs/heads/main"),
        summary: "the ref allowed to renew leases",
    },
    Binding {
        name: names::CACHET_DEPLOY_NAME,
        kind: BindingKind::Var,
        requirement: Requirement::Derived,
        env: None,
        default: None,
        summary: "the deployment's name, which the console prints in its header",
    },
    Binding {
        name: names::CACHET_GC_CRON,
        kind: BindingKind::Var,
        requirement: Requirement::Derived,
        env: None,
        default: None,
        summary: "the collector's schedule, the same expression its trigger fires on",
    },
    Binding {
        name: names::CACHET_GC_GRACE_MS,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: Some(env::GC_GRACE_MS),
        default: None,
        summary: "the grace window override; unset takes the worker's fourteen days",
    },
    Binding {
        name: names::CACHET_GC_ARMED,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: None,
        default: None,
        summary: "0 disarms the collector; only the test lanes set it",
    },
    Binding {
        name: names::CACHET_FONT_CSS,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: Some(env::FONT_CSS),
        default: None,
        summary: "a stylesheet the console loads for licensed faces; unset ships the free ones",
    },
    Binding {
        name: names::CACHET_STATS_DATASET,
        kind: BindingKind::Var,
        requirement: Requirement::Derived,
        env: None,
        default: None,
        summary: "the Analytics Engine dataset the deployment writes and reads back",
    },
    Binding {
        name: names::CLOUDFLARE_ACCOUNT_ID,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: Some(env::CLOUDFLARE_ACCOUNT_ID),
        default: None,
        summary: "the account the counter route queries under; the stats token needs it",
    },
    Binding {
        name: names::CACHET_STATS_API_URL,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: None,
        default: None,
        summary: "where the counter route sends its statement; only the lanes set it",
    },
    Binding {
        name: names::CACHET_JWKS_URL,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: None,
        default: None,
        summary: "GitHub's JWKS document; only the lanes set it",
    },
    Binding {
        name: names::CACHET_GITHUB_API_URL,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: None,
        default: None,
        summary: "the GitHub REST API root; only the lanes set it",
    },
    Binding {
        name: names::CACHET_GITHUB_WEB_URL,
        kind: BindingKind::Var,
        requirement: Requirement::Optional,
        env: None,
        default: None,
        summary: "the GitHub web origin; only the lanes set it",
    },
    Binding {
        name: names::CACHET_SIGNING_KEY,
        kind: BindingKind::Secret,
        requirement: Requirement::Required,
        env: Some(env::SIGNING_KEY),
        default: None,
        summary: "the signing key, <host>-1:<base64>, written by cachet keygen",
    },
    Binding {
        name: names::CACHET_OAUTH_CLIENT_SECRET,
        kind: BindingKind::Secret,
        requirement: Requirement::Required,
        env: Some(env::OAUTH_CLIENT_SECRET),
        default: None,
        summary: "the GitHub OAuth App's client secret",
    },
    Binding {
        name: names::CACHET_STATS_TOKEN,
        kind: BindingKind::Secret,
        requirement: Requirement::Optional,
        env: Some(env::STATS_TOKEN),
        default: None,
        summary: "a Cloudflare token scoped to Account Analytics:Read; without it the deployment counts and cannot report",
    },
    Binding {
        name: names::CACHE_BUCKET,
        kind: BindingKind::Platform(PlatformKind::R2Bucket),
        requirement: Requirement::Required,
        env: None,
        default: None,
        summary: "the R2 bucket the whole cache lives in",
    },
    Binding {
        name: names::CACHET_KV,
        kind: BindingKind::Platform(PlatformKind::KvNamespace),
        requirement: Requirement::Required,
        env: None,
        default: None,
        summary: "the KV namespace for verdicts, sessions, and OAuth state (ADR 0003)",
    },
    Binding {
        name: names::CACHET_EVENTS,
        kind: BindingKind::Platform(PlatformKind::AnalyticsEngine),
        requirement: Requirement::Required,
        env: None,
        default: None,
        summary: "the dataset every read, write, and probe is counted into",
    },
    Binding {
        name: names::ASSETS,
        kind: BindingKind::Platform(PlatformKind::Assets),
        requirement: Requirement::Required,
        env: None,
        default: None,
        summary: "the asset layer the console serves from (ADR 0014)",
    },
];

/// Looks a binding up by name.
#[must_use]
pub fn binding(name: &str) -> Option<&'static Binding> {
    BINDINGS.iter().find(|candidate| candidate.name == name)
}

/// Every var, in roster order.
pub fn vars() -> impl Iterator<Item = &'static Binding> {
    BINDINGS
        .iter()
        .filter(|binding| binding.kind == BindingKind::Var)
}

/// Every secret, in roster order.
pub fn secrets() -> impl Iterator<Item = &'static Binding> {
    BINDINGS
        .iter()
        .filter(|binding| binding.kind == BindingKind::Secret)
}

/// Every platform binding, in roster order.
pub fn platform() -> impl Iterator<Item = &'static Binding> {
    BINDINGS
        .iter()
        .filter(|binding| matches!(binding.kind, BindingKind::Platform(_)))
}

/// Whether a deploy ever sets this binding: it has a deploy-time source
/// or the grammar derives it. The overrides only the lanes set answer
/// false, so a plan and a manifest leave them out.
#[must_use]
pub fn is_deployable(binding: &Binding) -> bool {
    binding.env.is_some() || binding.requirement == Requirement::Derived
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_appears_once() {
        let mut names: Vec<&str> = BINDINGS.iter().map(|binding| binding.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "a duplicated name is two homes again");
    }

    #[test]
    fn every_env_spelling_appears_once() {
        let mut spellings: Vec<&str> = BINDINGS.iter().filter_map(|binding| binding.env).collect();
        let total = spellings.len();
        spellings.sort_unstable();
        spellings.dedup();
        assert_eq!(
            spellings.len(),
            total,
            "one environment variable cannot supply two bindings"
        );
    }

    #[test]
    fn a_derived_or_platform_binding_takes_no_environment_and_no_default() {
        for binding in BINDINGS {
            let computed = binding.requirement == Requirement::Derived
                || matches!(binding.kind, BindingKind::Platform(_));
            if computed {
                assert!(
                    binding.env.is_none(),
                    "{}: nothing supplies it",
                    binding.name
                );
                assert!(
                    binding.default.is_none(),
                    "{}: nothing defaults it",
                    binding.name
                );
            }
        }
    }

    #[test]
    fn only_a_required_var_carries_a_default() {
        // An optional var with a default would always be set, which makes
        // it required in fact; a secret with a default would be a
        // predictable secret.
        for binding in BINDINGS {
            if binding.default.is_some() {
                assert_eq!(binding.kind, BindingKind::Var, "{}", binding.name);
                assert_eq!(
                    binding.requirement,
                    Requirement::Required,
                    "{}",
                    binding.name
                );
            }
        }
    }

    #[test]
    fn a_secret_is_never_derived() {
        for binding in secrets() {
            assert_ne!(
                binding.requirement,
                Requirement::Derived,
                "{}: a derived secret is a predictable secret",
                binding.name
            );
        }
    }

    #[test]
    fn every_summary_reads_as_one_line() {
        for binding in BINDINGS {
            assert!(
                !binding.summary.is_empty() && binding.summary.len() <= 120,
                "{}: a summary is one readable line",
                binding.name
            );
        }
    }

    #[test]
    fn the_named_constants_are_in_the_roster() {
        for name in [
            names::CACHE_BUCKET,
            names::CACHET_KV,
            names::CACHET_EVENTS,
            names::ASSETS,
            names::CACHET_SIGNING_KEY,
        ] {
            assert!(binding(name).is_some(), "{name} is rostered");
        }
    }

    #[test]
    fn the_lane_overrides_are_not_deployable() {
        for name in [
            names::CACHET_GC_ARMED,
            names::CACHET_STATS_API_URL,
            names::CACHET_JWKS_URL,
            names::CACHET_GITHUB_API_URL,
            names::CACHET_GITHUB_WEB_URL,
        ] {
            let found = binding(name).expect("rostered");
            assert!(!is_deployable(found), "{name} is a lane override");
        }
        assert!(is_deployable(
            binding(names::CACHET_HOST).expect("rostered")
        ));
        assert!(is_deployable(
            binding(names::CACHET_GC_CRON).expect("rostered")
        ));
    }
}
