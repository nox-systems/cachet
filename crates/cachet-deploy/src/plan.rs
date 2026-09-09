//! The plan: what `plan(name, env)` answers, and the one function that
//! answers it.
//!
//! A plan is secret-free by construction. It carries names, a host, and
//! the vars a worker reads, and it carries no value the environment map
//! holds under a secret binding. That is what makes a plan safe to print,
//! paste into a pull request, and read, and the property lane asserts it
//! over arbitrary maps rather than trusting the shape.
//!
//! Pure: `plan` reads its arguments and nothing else. Two runs on two
//! machines with the same inputs produce byte-identical output, which the
//! golden lane pins.

use serde::Serialize;

use crate::env::{self, EnvMap};
use crate::error::DeployError;
use crate::name::Deployment;
use crate::roster::{self, Requirement, names};
use crate::spec;

/// The worker half of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerPlan {
    /// The worker's name.
    pub name: String,
    /// The platform semantics this deployment pins.
    pub compatibility_date: String,
    /// The compatibility flags, stated rather than inherited.
    pub compatibility_flags: Vec<String>,
    /// The collector's schedule.
    pub cron: String,
    /// The CPU ceiling in milliseconds.
    pub cpu_ms: u32,
    /// Whether observability is on.
    pub observability: bool,
}

/// One var the plan decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedVar {
    /// The binding's name.
    pub name: String,
    /// Its value. Secret-free: only vars appear here.
    pub value: String,
    /// Whether the grammar derived it or the operator supplied it.
    pub derived: bool,
}

/// A whole deployment plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployPlan {
    /// The deployment's name as the operator wrote it.
    pub name: String,
    /// The name every account resource is created under.
    pub resource_name: String,
    /// The cache's host, which is also the signing key's name.
    pub host: String,
    /// The custom domain attached to the worker: the host unless
    /// overridden.
    pub domain: String,
    /// The R2 bucket.
    pub bucket: String,
    /// The KV namespace's title.
    pub kv_title: String,
    /// The Analytics Engine dataset.
    pub dataset: String,
    /// The worker.
    pub worker: WorkerPlan,
    /// Every var, in roster order.
    pub vars: Vec<PlannedVar>,
    /// The secret bindings this deployment expects, by name only.
    pub secrets: Vec<String>,
    /// The environment file this deployment's configuration lives in.
    pub env_file: String,
}

/// Builds a plan.
///
/// The refusals are `infra/src/config.ts`'s, in its order: the name
/// grammar, then every missing required value at once, then the host's
/// shape, then the stats token's dependency on the account id. Two are
/// added here because a plan should refuse a typo the platform would
/// otherwise refuse later with a worse message: a grace window that is
/// not a number, and a domain override that is not a hostname.
///
/// # Errors
/// [`DeployError`], one variant per shape the operator got wrong. No
/// variant carries a value from `env`, only names.
///
/// # Examples
/// ```
/// use cachet_deploy::env::EnvMap;
/// use cachet_deploy::roster::env as deploy;
/// let env = EnvMap::from_pairs([
///     (deploy::HOST, "cache.example.com"),
///     (deploy::ORGS, "acme, acme-labs"),
///     (deploy::ADMINS, "ada"),
///     (deploy::OAUTH_CLIENT_ID, "Iv1.abc"),
///     (deploy::SIGNING_KEY, "cache.example.com-1:secret"),
///     (deploy::OAUTH_CLIENT_SECRET, "shh"),
/// ]);
/// let plan = cachet_deploy::plan("production", &env)?;
/// assert_eq!(plan.resource_name, "cachet-production");
/// assert_eq!(plan.dataset, "cachet_production");
/// // The comma list is normalized, and the defaults are filled in.
/// let orgs = plan.vars.iter().find(|v| v.name == "CACHET_ORGS").expect("planned");
/// assert_eq!(orgs.value, "acme,acme-labs");
/// let audience = plan.vars.iter().find(|v| v.name == "CACHET_AUDIENCE").expect("planned");
/// assert_eq!(audience.value, "cachet");
/// // Secrets are named and never carried.
/// assert!(plan.secrets.contains(&"CACHET_SIGNING_KEY".to_owned()));
/// assert!(!serde_json::to_string(&plan)?.contains("shh"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn plan(name: &str, env: &EnvMap) -> Result<DeployPlan, DeployError> {
    let deployment = Deployment::new(name)?;
    refuse_missing(env)?;
    let host = required_host(env)?;
    let domain = match env.get(env::DOMAIN) {
        Some(override_) => plausible_host(override_).ok_or(DeployError::Malformed(env::DOMAIN))?,
        None => host.clone(),
    };
    let vars = planned_vars(&deployment, env, &host)?;
    let secrets = planned_secrets(env)?;

    Ok(DeployPlan {
        name: deployment.name().to_owned(),
        resource_name: deployment.resource_name(),
        host,
        domain,
        bucket: deployment.bucket_name(),
        kv_title: deployment.kv_title(),
        dataset: deployment.dataset(),
        worker: WorkerPlan {
            name: deployment.worker_name(),
            compatibility_date: spec::COMPATIBILITY_DATE.to_owned(),
            compatibility_flags: spec::COMPATIBILITY_FLAGS
                .iter()
                .map(|flag| (*flag).to_owned())
                .collect(),
            cron: spec::GC_CRON.to_owned(),
            cpu_ms: spec::CPU_MS,
            observability: spec::OBSERVABILITY,
        },
        vars,
        secrets,
        env_file: deployment.env_file_path(),
    })
}

/// Every required value with no default that the environment lacks, named
/// at once.
fn refuse_missing(env: &EnvMap) -> Result<(), DeployError> {
    let missing: Vec<&'static str> = roster::BINDINGS
        .iter()
        .filter(|binding| binding.requirement == Requirement::Required)
        .filter(|binding| binding.default.is_none())
        .filter_map(|binding| binding.env)
        .filter(|env_name| !env.has(env_name))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(DeployError::Missing(missing))
    }
}

/// The host, present and shaped like a hostname.
fn required_host(env: &EnvMap) -> Result<String, DeployError> {
    let raw = env
        .get(roster::env::HOST)
        .ok_or_else(|| DeployError::Missing(vec![roster::env::HOST]))?;
    plausible_host(raw).ok_or(DeployError::Malformed(roster::env::HOST))
}

/// `^[a-z0-9][a-z0-9.-]*[a-z0-9]$` with no `..`, the check
/// `infra/src/config.ts` applied. Lowercase only, because the host becomes
/// the signing key's name and appears in every signature exactly as
/// written.
fn plausible_host(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return None;
    };
    let edge = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !edge(first) || !edge(last) || bytes.len() < 2 {
        return None;
    }
    if !bytes
        .iter()
        .all(|byte| edge(byte) || *byte == b'.' || *byte == b'-')
    {
        return None;
    }
    if raw.contains("..") {
        return None;
    }
    Some(raw.to_owned())
}

/// Normalize a comma list: trim every entry, drop empties, re-join.
fn comma_list(raw: &str) -> String {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Every var the plan decides, in roster order.
///
/// A derived var is computed. A var with a deploy-time source takes the
/// environment's value, normalized where the worker parses a list, or its
/// default, or nothing when it is optional and unset. A var with no source
/// is one only the lanes set, and a deployment never carries it.
fn planned_vars(
    deployment: &Deployment,
    env: &EnvMap,
    host: &str,
) -> Result<Vec<PlannedVar>, DeployError> {
    let mut planned = Vec::new();
    for binding in roster::vars() {
        if binding.requirement == Requirement::Derived {
            planned.push(PlannedVar {
                name: binding.name.to_owned(),
                value: derived_value(deployment, binding.name),
                derived: true,
            });
            continue;
        }
        let Some(env_name) = binding.env else {
            continue;
        };
        let value = match (env.get(env_name), binding.default) {
            (Some(value), _) => shaped(binding.name, env_name, value)?,
            (None, Some(default)) => default.to_owned(),
            (None, None) => continue,
        };
        planned.push(PlannedVar {
            name: binding.name.to_owned(),
            value,
            derived: false,
        });
    }
    // why here and not in the loop: the token is a secret and the account
    // id is a var, so the dependency crosses the two walks.
    if env.has(roster::env::STATS_TOKEN) && !env.has(roster::env::CLOUDFLARE_ACCOUNT_ID) {
        return Err(DeployError::Requires {
            present: roster::env::STATS_TOKEN,
            absent: roster::env::CLOUDFLARE_ACCOUNT_ID,
        });
    }
    debug_assert!(
        planned
            .iter()
            .any(|var| var.name == names::CACHET_HOST && var.value == host),
        "the host var is the validated host"
    );
    Ok(planned)
}

/// What the grammar computes rather than reads.
fn derived_value(deployment: &Deployment, name: &str) -> String {
    match name {
        names::CACHET_DEPLOY_NAME => deployment.name().to_owned(),
        names::CACHET_GC_CRON => spec::GC_CRON.to_owned(),
        names::CACHET_STATS_DATASET => deployment.dataset(),
        other => unreachable!("{other} is rostered as derived and has no derivation"),
    }
}

/// A supplied value, shaped the way the worker will read it.
fn shaped(name: &str, env_name: &'static str, value: &str) -> Result<String, DeployError> {
    match name {
        names::CACHET_ORGS | names::CACHET_ADMINS => Ok(comma_list(value)),
        names::CACHET_HOST => plausible_host(value).ok_or(DeployError::Malformed(env_name)),
        names::CACHET_GC_GRACE_MS => value
            .parse::<u64>()
            .map(|_| value.to_owned())
            .map_err(|_| DeployError::Malformed(env_name)),
        _ => Ok(value.to_owned()),
    }
}

/// The secret bindings this deployment expects, by name only.
fn planned_secrets(env: &EnvMap) -> Result<Vec<String>, DeployError> {
    let mut expected = Vec::new();
    for binding in roster::secrets() {
        let Some(env_name) = binding.env else {
            continue;
        };
        match (env.has(env_name), binding.requirement) {
            (true, _) => expected.push(binding.name.to_owned()),
            (false, Requirement::Required) => {
                return Err(DeployError::Missing(vec![env_name]));
            }
            (false, _) => {}
        }
    }
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::env as deploy;

    fn minimum() -> EnvMap {
        EnvMap::from_pairs([
            (deploy::HOST, "cache.example.com"),
            (deploy::ORGS, "acme"),
            (deploy::ADMINS, "ada"),
            (deploy::OAUTH_CLIENT_ID, "Iv1.abc"),
            (deploy::SIGNING_KEY, "cache.example.com-1:secret"),
            (deploy::OAUTH_CLIENT_SECRET, "shh"),
        ])
    }

    fn with(base: &EnvMap, extra: &[(&str, &str)]) -> EnvMap {
        let mut pairs: Vec<(String, String)> = base
            .names()
            .map(|name| {
                (
                    name.to_owned(),
                    base.get(name).unwrap_or_default().to_owned(),
                )
            })
            .collect();
        for (name, value) in extra {
            pairs.push(((*name).to_owned(), (*value).to_owned()));
        }
        EnvMap::from_pairs(pairs)
    }

    #[test]
    fn every_missing_value_is_named_at_once() {
        let refusal = plan("production", &EnvMap::default()).unwrap_err();
        assert_eq!(
            refusal,
            DeployError::Missing(vec![
                deploy::HOST,
                deploy::ORGS,
                deploy::ADMINS,
                deploy::OAUTH_CLIENT_ID,
                deploy::SIGNING_KEY,
                deploy::OAUTH_CLIENT_SECRET,
            ])
        );
    }

    #[test]
    fn the_defaults_fill_in_and_an_override_wins() {
        let built = plan("production", &minimum()).expect("plans");
        let value = |name: &str| {
            built
                .vars
                .iter()
                .find(|var| var.name == name)
                .map(|var| var.value.clone())
        };
        assert_eq!(value(names::CACHET_AUDIENCE).as_deref(), Some("cachet"));
        assert_eq!(
            value(names::CACHET_DEFAULT_BRANCH_REF).as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(value(names::CACHET_GC_GRACE_MS), None);
        assert_eq!(value(names::CACHET_FONT_CSS), None);
        assert_eq!(
            value(names::CACHET_GC_ARMED),
            None,
            "a lane override is never planned"
        );

        let tuned = plan(
            "production",
            &with(
                &minimum(),
                &[(deploy::AUDIENCE, "other"), (deploy::GC_GRACE_MS, "0")],
            ),
        )
        .expect("plans");
        let audience = tuned
            .vars
            .iter()
            .find(|var| var.name == names::CACHET_AUDIENCE);
        assert_eq!(audience.map(|var| var.value.as_str()), Some("other"));
        let grace = tuned
            .vars
            .iter()
            .find(|var| var.name == names::CACHET_GC_GRACE_MS);
        assert_eq!(grace.map(|var| var.value.as_str()), Some("0"));
    }

    #[test]
    fn the_derived_vars_hang_off_the_name() {
        let built = plan("cachet-staging", &minimum()).expect("plans");
        let derived: Vec<(&str, &str)> = built
            .vars
            .iter()
            .filter(|var| var.derived)
            .map(|var| (var.name.as_str(), var.value.as_str()))
            .collect();
        assert_eq!(
            derived,
            [
                (names::CACHET_DEPLOY_NAME, "cachet-staging"),
                (names::CACHET_GC_CRON, "0 * * * *"),
                (names::CACHET_STATS_DATASET, "cachet_staging"),
            ]
        );
    }

    #[test]
    fn the_domain_is_the_host_unless_overridden() {
        assert_eq!(
            plan("production", &minimum()).expect("plans").domain,
            "cache.example.com"
        );
        let overridden = with(&minimum(), &[(env::DOMAIN, "cache2.example.com")]);
        assert_eq!(
            plan("production", &overridden).expect("plans").domain,
            "cache2.example.com"
        );
        let bad = with(&minimum(), &[(env::DOMAIN, "Cache.Example.com")]);
        assert_eq!(
            plan("production", &bad).unwrap_err(),
            DeployError::Malformed(env::DOMAIN)
        );
    }

    #[test]
    fn the_host_is_shape_checked() {
        for bad in [
            "-cache.example.com",
            "cache.example.com-",
            "cache..example.com",
            "Cache.example.com",
            "a",
        ] {
            let env = with(&minimum(), &[(deploy::HOST, bad)]);
            assert_eq!(
                plan("production", &env).unwrap_err(),
                DeployError::Malformed(deploy::HOST),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_stats_token_needs_the_account_id() {
        let half = with(&minimum(), &[(deploy::STATS_TOKEN, "token")]);
        assert_eq!(
            plan("production", &half).unwrap_err(),
            DeployError::Requires {
                present: deploy::STATS_TOKEN,
                absent: deploy::CLOUDFLARE_ACCOUNT_ID,
            }
        );
        let whole = with(
            &minimum(),
            &[
                (deploy::STATS_TOKEN, "token"),
                (
                    deploy::CLOUDFLARE_ACCOUNT_ID,
                    "0123456789abcdef0123456789abcdef",
                ),
            ],
        );
        let built = plan("production", &whole).expect("plans");
        assert!(
            built
                .secrets
                .contains(&names::CACHET_STATS_TOKEN.to_owned())
        );
        assert!(
            built
                .vars
                .iter()
                .any(|var| var.name == names::CLOUDFLARE_ACCOUNT_ID)
        );
        // The account id alone is allowed, the way config.ts allowed it.
        let alone = with(
            &minimum(),
            &[(
                deploy::CLOUDFLARE_ACCOUNT_ID,
                "0123456789abcdef0123456789abcdef",
            )],
        );
        assert!(plan("production", &alone).is_ok());
    }

    #[test]
    fn a_grace_window_that_is_not_a_number_refuses_by_name() {
        let env = with(&minimum(), &[(deploy::GC_GRACE_MS, "fourteen days")]);
        assert_eq!(
            plan("production", &env).unwrap_err(),
            DeployError::Malformed(deploy::GC_GRACE_MS)
        );
    }
}
