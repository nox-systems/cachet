//! The CI driver: turns the job environment into pipeline inputs and
//! renders the pipeline's events as the `cachet:` log vocabulary. The
//! composite action exports the CACHET_* variables; this module resolves,
//! never the other way around, so running the binary standalone answers
//! the same missing-input diagnosis the action would.
//!
//! The contract on failure is the previous post step's: log and exit
//! zero. A broken push must never break the build it rides on.

use cachet_push::pipeline::PushEvent;

use crate::CliError;

/// Every variable the post phase reads, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushEnv {
    /// Where the cache lives.
    pub cache_url: String,
    /// The OIDC audience.
    pub audience: String,
    /// The lease project (`owner-repo`).
    pub project: String,
    /// Whitespace-split flake installables.
    pub installables: Vec<String>,
    /// GITHUB_REF == CACHET_DEFAULT_BRANCH_REF, both nonempty.
    pub is_default_branch: bool,
    /// RUNNER_TEMP, where the snapshot between main and post lives.
    pub runner_temp: std::path::PathBuf,
}

/// What resolving the environment concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvResolution {
    /// `CACHET_PUSH=false`: the job read-only.
    Disabled,
    /// Everything needed is present.
    Ready(PushEnv),
    /// These variables are unset or empty.
    Missing(Vec<String>),
}

/// The default renewal ref, when the job does not say.
const DEFAULT_BRANCH_REF: &str = "refs/heads/main";

/// Resolve the job environment. Resolution is data-in, answers-out: the
/// caller collects `std::env::vars` once.
#[must_use]
pub fn resolve_env(vars: &[(String, String)]) -> EnvResolution {
    let value = |name: &str| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    if value("CACHET_PUSH").as_deref() == Some("false") {
        return EnvResolution::Disabled;
    }
    let cache_url = value("CACHET_CACHE_URL");
    let audience = value("CACHET_AUDIENCE");
    let project = value("CACHET_PROJECT");
    let missing: Vec<String> = [
        ("CACHET_CACHE_URL", cache_url.is_none()),
        ("CACHET_AUDIENCE", audience.is_none()),
        ("CACHET_PROJECT", project.is_none()),
    ]
    .into_iter()
    .filter(|(_, absent)| *absent)
    .map(|(name, _)| name.to_string())
    .collect();
    let (Some(cache_url), Some(audience), Some(project)) = (cache_url, audience, project) else {
        return EnvResolution::Missing(missing);
    };
    let default_ref =
        value("CACHET_DEFAULT_BRANCH_REF").unwrap_or_else(|| DEFAULT_BRANCH_REF.to_string());
    let github_ref = value("GITHUB_REF");
    let is_default_branch = matches!(github_ref, Some(actual) if actual == default_ref);
    let installables = value("CACHET_ROOTS")
        .map(|roots| roots.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let runner_temp = value("RUNNER_TEMP").map_or_else(
        || std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from,
    );
    EnvResolution::Ready(PushEnv {
        cache_url,
        audience,
        project,
        installables,
        is_default_branch,
        runner_temp,
    })
}

/// The missing-input diagnosis, verbatim the previous post step's shape:
/// one sentence naming what to set and who normally sets it.
#[must_use]
pub fn missing_message(missing: &[String]) -> String {
    format!(
        "cachet: nothing pushed, because {} {} unset. The cachet setup action exports these to the job environment; if you are running cachet push directly, set them yourself.",
        missing.join(", "),
        if missing.len() == 1 { "is" } else { "are" },
    )
}

/// The `cachet:` vocabulary: one line per event, matching the previous
/// post step's wording so existing log scanners keep working.
#[must_use]
pub fn render_event(event: &PushEvent) -> String {
    match event {
        PushEvent::SnapshotTaken => "cachet: store snapshot taken".to_string(),
        PushEvent::MainSnapshotFailed { message } => {
            format!("cachet: could not snapshot the store, so nothing will be pushed. {message}")
        }
        PushEvent::NothingAdded => "cachet: the job added nothing to the store".to_string(),
        PushEvent::InstallableUnresolved { installable } => {
            format!("cachet: could not resolve {installable}; it will not be a lease root")
        }
        PushEvent::ProbeBulkFailed { message } => {
            format!(
                "cachet: the presence probe failed, so every candidate pushes as absent: {message}"
            )
        }
        PushEvent::CacheTally {
            to_upload,
            cache_hits,
            unparseable_paths,
        } => tally_line(
            &format!("{to_upload} new to cachet, {cache_hits} already cached"),
            *unparseable_paths,
        ),
        PushEvent::UploadedObjects { count } => format!("cachet: uploaded {count} objects"),
        PushEvent::LeaseSkippedNotDefaultBranch => {
            "cachet: not the default branch, so the lease is not renewed".to_string()
        }
        PushEvent::LeaseRenewed { project } => format!("cachet: lease renewed for {project}"),
    }
}

fn tally_line(base: &str, unparseable_paths: usize) -> String {
    if unparseable_paths == 0 {
        format!("cachet: {base}")
    } else {
        format!("cachet: {base}, {unparseable_paths} unparseable (kept)")
    }
}

/// The main step: snapshot the store to the hand-off file. Failures are
/// reported and swallowed; the post step treats a missing file as an
/// empty before-set.
pub async fn run_snapshot(
    vars: &[(String, String)],
    commands: &cachet_push::real::TokioCommands,
    tell: &mut dyn FnMut(&str),
) {
    let runner_temp = vars
        .iter()
        .find(|(key, _)| key == "RUNNER_TEMP")
        .map_or_else(
            || std::path::PathBuf::from("/tmp"),
            |(_, v)| std::path::PathBuf::from(v),
        );
    let path = cachet_push::adapters::snapshot_path(&runner_temp);
    let mut sink = |event: PushEvent| tell(&render_event(&event));
    match cachet_push::pipeline::main_snapshot(commands, &mut sink).await {
        Ok(Some(text)) => {
            if let Err(failure) = std::fs::write(&path, text) {
                tell(&format!(
                    "cachet: could not write the snapshot file {}: {failure}, so nothing will be pushed",
                    path.display()
                ));
            }
        }
        Ok(None) => {}
        Err(failure) => {
            tell(&format!(
                "cachet: the snapshot failed unexpectedly: {failure}"
            ));
        }
    }
}

/// The post step: the whole pipeline, never failing the job.
pub async fn run_push(vars: &[(String, String)], tell: &mut dyn FnMut(&str)) {
    match resolve_env(vars) {
        EnvResolution::Disabled => {
            tell("cachet: pushing is disabled for this job");
        }
        EnvResolution::Missing(missing) => {
            tell(&missing_message(&missing));
        }
        EnvResolution::Ready(env) => run_pipeline(env, vars, tell).await,
    }
}

/// Wire the real adapters and drive. Every failure ends as one
/// `cachet:` line; the exit code stays zero by the caller's contract.
async fn run_pipeline(env: PushEnv, vars: &[(String, String)], tell: &mut dyn FnMut(&str)) {
    if let Err(failure) = run_pipeline_inner(env, vars, tell).await {
        tell(&format!("cachet: {failure}"));
    }
}

async fn run_pipeline_inner(
    env: PushEnv,
    vars: &[(String, String)],
    tell: &mut dyn FnMut(&str),
) -> Result<(), CliError> {
    let oidc_env =
        cachet_push::oidc::oidc_env(vars).map_err(|failure| CliError(failure.to_string()))?;
    let http =
        cachet_push::real::ReqwestHttp::new().map_err(|failure| CliError(failure.to_string()))?;
    let commands = cachet_push::real::TokioCommands;
    // why: one mint rides the whole run; the pipeline's 401 hook
    // invalidates it when the API says it aged.
    let oidc = cachet_push::oidc::OidcTokens::new(&oidc_env, &http);
    let tokens = cachet_push::oidc::RunTokens::over(&oidc);
    let sleep = |ms: u64| -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_millis(ms)))
    };
    let adapters = cachet_push::adapters::Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let before = std::fs::read_to_string(cachet_push::adapters::snapshot_path(&env.runner_temp))
        .unwrap_or_default();
    // why: the staging tree exists only for the `nix copy` half of this
    // run; it dies with the guard.
    let staging = tempfile::tempdir()
        .map_err(|failure| CliError(format!("could not make the staging directory: {failure}")))?;
    let inputs = cachet_push::pipeline::PushInputs {
        cache_url: env.cache_url,
        audience: env.audience,
        project: env.project,
        installables: env.installables,
        is_default_branch: env.is_default_branch,
    };
    let mut sink = |event: PushEvent| tell(&render_event(&event));
    cachet_push::pipeline::push(&adapters, &inputs, &before, staging.path(), &mut sink)
        .await
        .map_err(|failure| CliError(failure.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn resolution_reports_exactly_what_is_missing() {
        let resolved = resolve_env(&vars(&[("CACHET_CACHE_URL", "https://cache.example.com")]));
        match resolved {
            EnvResolution::Missing(missing) => {
                assert_eq!(missing, vec!["CACHET_AUDIENCE", "CACHET_PROJECT"]);
            }
            other => panic!("expected Missing, got {other:?}"),
        }
        assert_eq!(
            missing_message(&["CACHET_PROJECT".to_string()]),
            "cachet: nothing pushed, because CACHET_PROJECT is unset. The cachet setup action exports these to the job environment; if you are running cachet push directly, set them yourself."
        );
    }

    #[test]
    fn resolution_defaults_and_branch_gate() {
        let resolved = resolve_env(&vars(&[
            ("CACHET_CACHE_URL", "https://cache.example.com"),
            ("CACHET_AUDIENCE", "cachet"),
            ("CACHET_PROJECT", "org-repo"),
            ("CACHET_ROOTS", ".#a  .#b\n.#c"),
            ("GITHUB_REF", "refs/heads/main"),
        ]));
        match resolved {
            EnvResolution::Ready(env) => {
                assert!(env.is_default_branch, "main matches the default ref");
                assert_eq!(env.installables, vec![".#a", ".#b", ".#c"]);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        let off = resolve_env(&vars(&[
            ("CACHET_CACHE_URL", "https://cache.example.com"),
            ("CACHET_AUDIENCE", "cachet"),
            ("CACHET_PROJECT", "org-repo"),
            ("GITHUB_REF", "refs/pull/7/merge"),
        ]));
        match off {
            EnvResolution::Ready(env) => assert!(!env.is_default_branch),
            other => panic!("expected Ready, got {other:?}"),
        }
        // An empty GITHUB_REF never matches: undefined answers keep out.
        let blank = resolve_env(&vars(&[
            ("CACHET_CACHE_URL", "https://cache.example.com"),
            ("CACHET_AUDIENCE", "cachet"),
            ("CACHET_PROJECT", "org-repo"),
        ]));
        match blank {
            EnvResolution::Ready(env) => assert!(!env.is_default_branch),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn disabled_wins_over_readiness() {
        let resolved = resolve_env(&vars(&[("CACHET_PUSH", "false")]));
        assert_eq!(resolved, EnvResolution::Disabled);
    }

    #[test]
    fn the_vocabulary_holds() {
        assert_eq!(
            render_event(&PushEvent::SnapshotTaken),
            "cachet: store snapshot taken"
        );
        assert_eq!(
            render_event(&PushEvent::NothingAdded),
            "cachet: the job added nothing to the store"
        );
        assert_eq!(
            render_event(&PushEvent::CacheTally {
                to_upload: 3,
                cache_hits: 1,
                unparseable_paths: 2,
            }),
            "cachet: 3 new to cachet, 1 already cached, 2 unparseable (kept)"
        );
        assert_eq!(
            render_event(&PushEvent::ProbeBulkFailed {
                message: "connection reset".to_string(),
            }),
            "cachet: the presence probe failed, so every candidate pushes as absent: connection reset"
        );
        assert_eq!(
            render_event(&PushEvent::UploadedObjects { count: 7 }),
            "cachet: uploaded 7 objects"
        );
        assert_eq!(
            render_event(&PushEvent::LeaseSkippedNotDefaultBranch),
            "cachet: not the default branch, so the lease is not renewed"
        );
        assert_eq!(
            render_event(&PushEvent::LeaseRenewed {
                project: "org-repo".to_string()
            }),
            "cachet: lease renewed for org-repo"
        );
        assert_eq!(
            render_event(&PushEvent::InstallableUnresolved {
                installable: ".#gone".to_string()
            }),
            "cachet: could not resolve .#gone; it will not be a lease root"
        );
    }
}
