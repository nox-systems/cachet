//! The release manifest: the contract between this repository's CI and
//! the deployer.
//!
//! A manifest describes one release of cachet as a template. Every
//! account-specific value is a placeholder from a closed set, and a
//! deployer renders the placeholders from an instance's own state before
//! it touches an account. The template alone is `deploy-manifest.json` at
//! the repository root, committed and drift-gated; the release workflow
//! adds the measured artifacts (module hashes, the asset map) and signs
//! the result.
//!
//! The placeholders:
//!
//! | Placeholder | Meaning |
//! | --- | --- |
//! | `$NAME` | the deployment's name as the operator wrote it |
//! | `$RESOURCE_NAME` | `cachet-<name>`, or the name when already prefixed |
//! | `$RESOURCE_NAME_UNDERSCORED` | the same with dashes as underscores |
//! | `$HOST` | the cache's host |
//! | `$VAR(<binding>)` | the operator's value for a var |
//! | `$SECRET(<binding>)` | the operator's value for a secret, never rendered into anything printable |
//! | `$VERSION` | this release's version |
//! | `$GENERATED_PUBLIC(<secret>)` | the public half of a key the deployer generated for a secret |
//!
//! A renderer fails closed on any `$` token outside this list, and
//! [`crate::spec::MANIFEST_VERSION`] guards the two sides.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::roster::{self, BindingKind, PlatformKind, Requirement, names};
use crate::spec::{self, ModuleKind};

/// The whole manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// The schema version the deployer must agree with.
    pub manifest_version: u32,
    /// The product.
    pub product: String,
    /// The release's version.
    pub version: String,
    /// The worker.
    pub worker: WorkerSpec,
    /// The account resources a deploy creates, each with a key the
    /// bindings reference.
    pub resources: Vec<Resource>,
    /// The worker's bindings.
    pub bindings: Vec<BindingSpec>,
    /// The vars a deployment carries, in roster order.
    pub vars: Vec<VarSpec>,
    /// The secrets a deployment carries, in roster order.
    pub secrets: Vec<SecretSpec>,
    /// The cron triggers.
    pub crons: Vec<CronSpec>,
    /// How the worker is reached.
    pub route: RouteSpec,
    /// The asset layer.
    pub assets: AssetsSpec,
    /// What a deploy checks after it applies, in order.
    pub verify: Vec<Probe>,
    /// The console.
    pub console: ConsoleSpec,
    /// The measured artifacts. Absent on the committed template; the
    /// release workflow fills it, and a deployer refuses a manifest
    /// without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Artifacts>,
}

/// The worker's release-level facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSpec {
    /// The entry module's name within `modules`.
    pub main_module: String,
    /// Every module the bundle uploads.
    pub modules: Vec<ModuleDecl>,
    /// The platform semantics.
    pub compatibility_date: String,
    /// The compatibility flags.
    pub compatibility_flags: Vec<String>,
    /// Whether observability is on.
    pub observability: bool,
    /// The limits.
    pub limits: Limits,
}

/// One module, by name and kind; its hash lives in [`Artifacts`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleDecl {
    /// A POSIX path relative to the entry.
    pub name: String,
    /// What it is.
    pub kind: ModuleKind,
}

/// The worker's limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// The CPU ceiling in milliseconds.
    pub cpu_ms: u32,
}

/// An account resource a deploy creates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resource {
    /// An R2 bucket.
    R2Bucket {
        /// The key bindings reference.
        key: String,
        /// The bucket's name, as a placeholder formula.
        name: String,
    },
    /// A KV namespace.
    KvNamespace {
        /// The key bindings reference.
        key: String,
        /// The namespace's title, as a placeholder formula.
        title: String,
    },
    /// An Analytics Engine dataset. Configuration only: nothing is created
    /// and nothing is destroyed with the deployment.
    AnalyticsEngine {
        /// The key bindings reference.
        key: String,
        /// The dataset's name, as a placeholder formula.
        dataset: String,
    },
}

/// One worker binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingSpec {
    /// The Workers API binding type.
    #[serde(rename = "type")]
    pub kind: String,
    /// The binding's name.
    pub name: String,
    /// The resource it binds, for a binding that binds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

/// One var a deployment carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VarSpec {
    /// The binding's name.
    pub name: String,
    /// Whether a deployment must carry it.
    pub requirement: Requirement,
    /// Its value as a placeholder formula or a literal.
    pub value: String,
    /// The deploy-time environment variable that supplies it, for a CLI
    /// reading an env file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// The value it takes when the operator omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// One line an operator reads beside the name.
    pub summary: String,
    /// How a wizard asks for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wizard: Option<Wizard>,
}

/// One secret a deployment carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretSpec {
    /// The binding's name.
    pub name: String,
    /// Whether a deployment must carry it.
    pub requirement: Requirement,
    /// Its value as a `$SECRET(...)` placeholder.
    pub value: String,
    /// The deploy-time environment variable that supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// One line an operator reads beside the name.
    pub summary: String,
    /// How a wizard asks for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wizard: Option<Wizard>,
    /// How a deployer generates it where the customer is, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<Generated>,
}

/// How a wizard collects a value. The shape Cloudflare OS's
/// `deploy-inputs.json` uses, because it is the one a form needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wizard {
    /// The field's label.
    pub label: String,
    /// One or two sentences under the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// The third-party console page where the value is made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_url: Option<String>,
    /// Ordered setup steps shown beside the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup_steps: Vec<String>,
    /// A redirect URI to show the user, with `$HOST` unresolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri_template: Option<String>,
}

/// A secret a deployer generates rather than asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Generated {
    /// An Ed25519 key in nix's `<name>:<base64(seed ‖ public)>` form
    /// (`cachet-crypto/src/ed25519.rs`).
    Ed25519NixKey {
        /// The key's name, as a placeholder formula.
        key_name: String,
    },
}

/// One cron trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronSpec {
    /// The expression.
    pub expression: String,
    /// The var that must carry the same expression, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
}

/// How the worker is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSpec {
    /// Whether a custom domain is `required`, `optional`, or `none`.
    pub custom_domain: String,
    /// Whether a deployment may answer on workers.dev instead.
    pub workers_dev: bool,
    /// Whether the host is the deployment's identity: it names the
    /// signing key and appears in every signature, so moving it later is
    /// a key rotation.
    pub host_is_identity: bool,
}

/// The asset layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetsSpec {
    /// Where the build lands, relative to the repository root.
    pub directory: String,
    /// The path the assets are served under.
    pub base: String,
    /// The layer's HTML handling mode.
    pub html_handling: String,
    /// The layer's not-found handling mode.
    pub not_found_handling: String,
}

/// One post-apply check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// The path to GET on the deployment's host.
    pub get: String,
    /// The status it must answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// A JSON path into the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_path: Option<String>,
    /// What the path must equal, as a placeholder formula or a literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
}

/// The console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleSpec {
    /// The path it is served under.
    pub path: String,
}

/// The measured artifacts of one release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifacts {
    /// Every module's hash and size, in `modules` order.
    pub modules: Vec<ModuleArtifact>,
    /// Every asset by served path.
    pub assets: BTreeMap<String, AssetArtifact>,
}

/// One module's measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleArtifact {
    /// The module's name.
    pub name: String,
    /// Its SHA-256, hex.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
}

/// One asset's measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetArtifact {
    /// The hash the asset upload API takes.
    pub hash: String,
    /// Its size in bytes.
    pub size: u64,
}

impl Manifest {
    /// The template for one release: everything but the measured
    /// artifacts, built from the roster and the release constants.
    ///
    /// # Examples
    /// ```
    /// let manifest = cachet_deploy::Manifest::template("0.1.2");
    /// assert_eq!(manifest.product, "cachet");
    /// assert!(manifest.artifacts.is_none());
    /// let host = manifest.vars.iter().find(|v| v.name == "CACHET_HOST").expect("rostered");
    /// assert_eq!(host.value, "$HOST");
    /// ```
    #[must_use]
    pub fn template(version: &str) -> Self {
        Self {
            manifest_version: spec::MANIFEST_VERSION,
            product: spec::PRODUCT.to_owned(),
            version: version.to_owned(),
            worker: WorkerSpec {
                main_module: spec::MAIN_MODULE.to_owned(),
                modules: spec::MODULES
                    .iter()
                    .map(|(name, kind)| ModuleDecl {
                        name: (*name).to_owned(),
                        kind: *kind,
                    })
                    .collect(),
                compatibility_date: spec::COMPATIBILITY_DATE.to_owned(),
                compatibility_flags: spec::COMPATIBILITY_FLAGS
                    .iter()
                    .map(|flag| (*flag).to_owned())
                    .collect(),
                observability: spec::OBSERVABILITY,
                limits: Limits {
                    cpu_ms: spec::CPU_MS,
                },
            },
            resources: roster::platform().filter_map(resource).collect(),
            bindings: roster::platform().map(binding_spec).collect(),
            vars: roster::vars()
                .filter(|binding| roster::is_deployable(binding))
                .map(var_spec)
                .collect(),
            secrets: roster::secrets()
                .filter(|binding| roster::is_deployable(binding))
                .map(secret_spec)
                .collect(),
            crons: vec![CronSpec {
                expression: spec::GC_CRON.to_owned(),
                var: Some(names::CACHET_GC_CRON.to_owned()),
            }],
            route: RouteSpec {
                custom_domain: "optional".to_owned(),
                workers_dev: true,
                host_is_identity: true,
            },
            assets: AssetsSpec {
                directory: spec::ASSETS_DIRECTORY.to_owned(),
                base: spec::CONSOLE_PATH.to_owned(),
                html_handling: spec::ASSETS_HTML_HANDLING.to_owned(),
                not_found_handling: spec::ASSETS_NOT_FOUND_HANDLING.to_owned(),
            },
            verify: vec![
                Probe {
                    get: "/nix-cache-info".to_owned(),
                    status: Some(200),
                    json_path: None,
                    equals: None,
                },
                Probe {
                    get: "/api/public/config".to_owned(),
                    status: None,
                    json_path: Some("$.version".to_owned()),
                    equals: Some("$VERSION".to_owned()),
                },
                Probe {
                    get: "/api/public/config".to_owned(),
                    status: None,
                    json_path: Some("$.publicKey".to_owned()),
                    equals: Some(format!("$GENERATED_PUBLIC({})", names::CACHET_SIGNING_KEY)),
                },
                Probe {
                    get: "/api/public/config".to_owned(),
                    status: None,
                    json_path: Some("$.deployment".to_owned()),
                    equals: Some("$NAME".to_owned()),
                },
            ],
            console: ConsoleSpec {
                path: spec::CONSOLE_PATH.to_owned(),
            },
            artifacts: None,
        }
    }
}

/// The resource key a platform binding references.
fn resource_key(kind: PlatformKind) -> &'static str {
    match kind {
        PlatformKind::R2Bucket => "bucket",
        PlatformKind::KvNamespace => "kv",
        PlatformKind::AnalyticsEngine => "events",
        PlatformKind::Assets => "assets",
    }
}

/// The resource a platform binding creates. The asset layer is uploaded
/// with the worker rather than created beside it.
fn resource(binding: &roster::Binding) -> Option<Resource> {
    let BindingKind::Platform(kind) = binding.kind else {
        return None;
    };
    let key = resource_key(kind).to_owned();
    match kind {
        PlatformKind::R2Bucket => Some(Resource::R2Bucket {
            key,
            name: "$RESOURCE_NAME".to_owned(),
        }),
        PlatformKind::KvNamespace => Some(Resource::KvNamespace {
            key,
            title: "$RESOURCE_NAME".to_owned(),
        }),
        PlatformKind::AnalyticsEngine => Some(Resource::AnalyticsEngine {
            key,
            dataset: "$RESOURCE_NAME_UNDERSCORED".to_owned(),
        }),
        PlatformKind::Assets => None,
    }
}

/// The binding a platform entry becomes, in the Workers API's words.
fn binding_spec(binding: &roster::Binding) -> BindingSpec {
    let BindingKind::Platform(kind) = binding.kind else {
        unreachable!("platform() yields platform bindings");
    };
    let (word, resource) = match kind {
        PlatformKind::R2Bucket => ("r2_bucket", Some(resource_key(kind))),
        PlatformKind::KvNamespace => ("kv_namespace", Some(resource_key(kind))),
        PlatformKind::AnalyticsEngine => ("analytics_engine", Some(resource_key(kind))),
        PlatformKind::Assets => ("assets", None),
    };
    BindingSpec {
        kind: word.to_owned(),
        name: binding.name.to_owned(),
        resource: resource.map(str::to_owned),
    }
}

/// A var's template value: a derivation or the operator's input.
fn var_value(binding: &roster::Binding) -> String {
    match binding.name {
        names::CACHET_HOST => "$HOST".to_owned(),
        names::CACHET_DEPLOY_NAME => "$NAME".to_owned(),
        names::CACHET_GC_CRON => spec::GC_CRON.to_owned(),
        names::CACHET_STATS_DATASET => "$RESOURCE_NAME_UNDERSCORED".to_owned(),
        other => format!("$VAR({other})"),
    }
}

fn var_spec(binding: &roster::Binding) -> VarSpec {
    VarSpec {
        name: binding.name.to_owned(),
        requirement: binding.requirement,
        value: var_value(binding),
        env: binding.env.map(str::to_owned),
        default: binding.default.map(str::to_owned),
        summary: binding.summary.to_owned(),
        wizard: wizard(binding.name),
    }
}

fn secret_spec(binding: &roster::Binding) -> SecretSpec {
    SecretSpec {
        name: binding.name.to_owned(),
        requirement: binding.requirement,
        value: format!("$SECRET({})", binding.name),
        env: binding.env.map(str::to_owned),
        summary: binding.summary.to_owned(),
        wizard: wizard(binding.name),
        generated: (binding.name == names::CACHET_SIGNING_KEY).then(|| Generated::Ed25519NixKey {
            key_name: "$HOST-1".to_owned(),
        }),
    }
}

/// How a wizard asks for each value an operator supplies. The words are
/// docs/DEPLOY.md's, so a form and the runbook say the same thing.
fn wizard(name: &str) -> Option<Wizard> {
    let plain = |label: &str, help: &str| {
        Some(Wizard {
            label: label.to_owned(),
            help: Some(help.to_owned()),
            console_url: None,
            setup_steps: Vec::new(),
            redirect_uri_template: None,
        })
    };
    match name {
        names::CACHET_HOST => plain(
            "Host",
            "The cache's address, in a zone this account holds. It names the signing key and appears in every signature, so choose it once: moving later is a key rotation.",
        ),
        names::CACHET_ORGS => plain(
            "GitHub organizations",
            "The org slugs this cache serves. CI in these orgs writes; their members read.",
        ),
        names::CACHET_ADMINS => plain(
            "Admins",
            "GitHub logins that see the console's collection and traffic screens.",
        ),
        names::CACHET_OAUTH_CLIENT_ID => Some(Wizard {
            label: "OAuth App client id".to_owned(),
            help: Some(
                "A GitHub OAuth App in the org, with Device Flow enabled. The callback URL is fixed by the host, so the app can exist before anything deploys."
                    .to_owned(),
            ),
            console_url: Some("https://github.com/settings/developers".to_owned()),
            setup_steps: vec![
                "In GitHub, create a new OAuth App (an OAuth App, not a GitHub App).".to_owned(),
                "Set the homepage URL to https://$HOST and the authorization callback URL to the redirect URI shown here.".to_owned(),
                "Enable Device Flow.".to_owned(),
                "Register the app, generate a client secret, and copy both values here.".to_owned(),
            ],
            redirect_uri_template: Some(format!("https://$HOST{}", spec::OAUTH_CALLBACK_PATH)),
        }),
        names::CACHET_OAUTH_CLIENT_SECRET => plain(
            "OAuth App client secret",
            "Generated in the OAuth App's settings. Shown once there; paste it here once.",
        ),
        names::CACHET_AUDIENCE => plain(
            "OIDC audience",
            "The audience CI tokens must carry. The action sends cachet unless told otherwise.",
        ),
        names::CACHET_DEFAULT_BRANCH_REF => plain(
            "Default branch ref",
            "The ref whose CI runs may renew leases. Pushes from other refs still land; they do not extend retention.",
        ),
        names::CACHET_GC_GRACE_MS => plain(
            "Grace window (milliseconds)",
            "How long an unreferenced path survives before the collector may delete it. Fourteen days unless set. Zero is for throwaway deployments.",
        ),
        names::CACHET_FONT_CSS => plain(
            "Font stylesheet",
            "A stylesheet serving licensed faces for the console. Unset ships the free ones.",
        ),
        names::CLOUDFLARE_ACCOUNT_ID => plain(
            "Cloudflare account id",
            "The account the counter route queries. Needed with the stats token.",
        ),
        names::CACHET_STATS_TOKEN => plain(
            "Stats token",
            "A Cloudflare API token with Account Analytics:Read. Without it the deployment counts and cannot report.",
        ),
        names::CACHET_SIGNING_KEY => plain(
            "Signing key",
            "Generated here. Download the secret: it is what lets you move to self-hosting or reinstall against a kept cache.",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_round_trips_through_json() {
        let template = Manifest::template("0.1.2");
        let json = serde_json::to_string(&template).expect("serializes");
        let back: Manifest = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, template);
    }

    #[test]
    fn every_placeholder_is_from_the_closed_set() {
        // The renderer fails closed on anything else, so a template that
        // wrote a fifth spelling would deploy nothing.
        let json = serde_json::to_string(&Manifest::template("0.1.2")).expect("serializes");
        let known = [
            "$NAME",
            "$RESOURCE_NAME_UNDERSCORED",
            "$RESOURCE_NAME",
            "$HOST-1",
            "$HOST",
            "$VAR(",
            "$SECRET(",
            "$VERSION",
            "$GENERATED_PUBLIC(",
        ];
        let mut rest = json.as_str();
        while let Some(at) = rest.find('$') {
            let tail = &rest[at..];
            // A `$.` is a JSON path in a probe, which is not a placeholder.
            let placeholder = tail.as_bytes().get(1).is_some_and(u8::is_ascii_uppercase);
            if placeholder {
                let matched = known.iter().find(|word| tail.starts_with(*word));
                assert!(
                    matched.is_some(),
                    "unknown placeholder at: {}",
                    &tail[..tail.len().min(40)]
                );
            }
            rest = &tail[1..];
        }
    }

    #[test]
    fn every_deployable_binding_is_in_the_template_and_no_other() {
        let template = Manifest::template("0.1.2");
        let listed: Vec<&str> = template
            .vars
            .iter()
            .map(|var| var.name.as_str())
            .chain(template.secrets.iter().map(|secret| secret.name.as_str()))
            .collect();
        for binding in roster::vars().chain(roster::secrets()) {
            assert_eq!(
                listed.contains(&binding.name),
                roster::is_deployable(binding),
                "{}",
                binding.name
            );
        }
    }

    #[test]
    fn every_platform_binding_references_a_declared_resource() {
        let template = Manifest::template("0.1.2");
        let keys: Vec<&str> = template
            .resources
            .iter()
            .map(|resource| match resource {
                Resource::R2Bucket { key, .. }
                | Resource::KvNamespace { key, .. }
                | Resource::AnalyticsEngine { key, .. } => key.as_str(),
            })
            .collect();
        for binding in &template.bindings {
            if let Some(resource) = &binding.resource {
                assert!(
                    keys.contains(&resource.as_str()),
                    "{} binds {resource}",
                    binding.name
                );
            }
        }
        assert_eq!(template.bindings.len(), 4);
        assert_eq!(template.resources.len(), 3);
    }

    #[test]
    fn the_cron_and_its_var_agree() {
        let template = Manifest::template("0.1.2");
        let cron = &template.crons[0];
        let var = template
            .vars
            .iter()
            .find(|var| Some(&var.name) == cron.var.as_ref())
            .expect("the cron names a var");
        assert_eq!(var.value, cron.expression);
    }
}
