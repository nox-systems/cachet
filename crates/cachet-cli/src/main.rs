//! The `cachet` binary: argv in, one command out. All policy lives in
//! the library modules; this file parses, wires the environment, and
//! picks the exit code.

use std::process::ExitCode;

use cachet_cli::config;
use clap::{Parser, Subcommand};

/// The cachet client: log in with GitHub, wire nix to the deployment,
/// push from CI.
#[derive(Parser)]
#[command(name = "cachet", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in to a deployment with GitHub's device flow and store the
    /// read token.
    Login {
        /// The cache's base URL (remembered for later commands).
        #[arg(long)]
        cache_url: String,
    },
    /// Write this machine's nix.conf and netrc so builds substitute from
    /// the cache, then restart the daemon. Works with both Determinate
    /// Nix and plain nix. Requires `cachet login` first.
    Setup {
        /// The cache's base URL; defaults to the one logged into.
        #[arg(long)]
        cache_url: Option<String>,
    },
    /// Revoke this machine's credential at the deployment and remove the
    /// local copy.
    Logout {
        /// The cache's base URL; defaults to the one logged into.
        #[arg(long)]
        cache_url: Option<String>,
    },
    /// Probe the read wiring against a deployment and report what holds.
    Doctor {
        /// The cache's base URL; defaults to the one logged into.
        #[arg(long)]
        cache_url: Option<String>,
    },
    /// Generate a deployment's ed25519 signing keypair. Bootstrap only.
    Keygen {
        /// The key's name: `<host>-1` of the deployment.
        #[arg(long)]
        name: String,
        /// Write `cachet-key.secret` (0600) and `cachet-key.public` here
        /// instead of printing the secret.
        #[arg(long)]
        out_dir: Option<std::path::PathBuf>,
    },
    /// CI mode: with --snapshot-only the composite's main step (snapshot
    /// the store); without it the post step (push what the job added).
    /// Reads the job's CACHET_* environment and always exits zero.
    Push {
        /// Only snapshot the store for a later post step.
        #[arg(long)]
        snapshot_only: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let vars: Vec<(String, String)> = std::env::vars().collect();
    match cli.command {
        Command::Login { cache_url } => run(&login(vars, cache_url).await),
        Command::Setup { cache_url } => run(&setup(vars, cache_url).await),
        Command::Logout { cache_url } => run(&logout(vars, cache_url).await),
        Command::Doctor { cache_url } => doctor(vars, cache_url).await,
        Command::Keygen { name, out_dir } => run(&keygen(&name, out_dir.as_deref())),
        Command::Push { snapshot_only } => {
            if snapshot_only {
                let commands = cachet_push::real::TokioCommands;
                cachet_cli::push::run_snapshot(&vars, &commands, &mut |line| println!("{line}"))
                    .await;
            } else {
                cachet_cli::push::run_push(&vars, &mut |line| println!("{line}")).await;
            }
            // The CI contract: a push failure is a log line, never a red job.
            ExitCode::SUCCESS
        }
    }
}

/// The shared failure-to-exit-code translation for ordinary commands.
fn run(result: &Result<(), cachet_cli::CliError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("cachet: {failure}");
            ExitCode::FAILURE
        }
    }
}

/// The login flow end to end: config from the deployment, flow against
/// GitHub, token into the state directory.
async fn login(vars: Vec<(String, String)>, cache_url: String) -> Result<(), cachet_cli::CliError> {
    let url = cache_url.trim_end_matches('/').to_string();
    let client = cachet_cli::http_client()?;
    let config = cachet_cli::login::fetch_public_config(&client, &url).await?;
    let server = cachet_cli::login::GithubLive::new(&client);
    let sleep = |ms: u64| -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_millis(ms)))
    };
    let (grant, who) =
        cachet_cli::login::run_device_flow(&server, &config.oauth_client_id, &sleep, &mut |line| {
            println!("{line}");
        })
        .await?;
    // The GitHub token's whole job is done here: it proves who you are
    // to the deployment once, and the deployment answers with a
    // credential of its own. What this machine keeps, and what the nix
    // daemon ends up carrying, is that one (ADR 0002). GitHub's token is
    // not stored, so its eight-hour lifetime never becomes this
    // machine's problem and the cache never holds anything replayable
    // against github.com.
    let issued = cachet_cli::login::exchange_for_read_token(&client, &url, &grant).await?;
    let dir = config::state_dir(&vars)?;
    config::store_login(&dir, &config.host, &url, &issued.token)?;
    println!("cachet: logged in to {} as {}", config.host, issued.login);
    println!(
        "cachet: this machine's credential expires {}; `cachet login` again renews it",
        cachet_cli::login::render_expiry(issued.expires_at_ms)
    );
    println!("cachet: run `cachet setup` to wire this machine's nix");
    debug_assert_eq!(who, issued.login, "the deployment names the same person");
    Ok(())
}

/// Logging out: the deployment forgets this machine's credential, then
/// the machine forgets it too. The order matters, because a local file
/// removed first would leave a credential nobody can name to revoke.
async fn logout(
    vars: Vec<(String, String)>,
    cache_url: Option<String>,
) -> Result<(), cachet_cli::CliError> {
    let dir = config::state_dir(&vars)?;
    let url = config::resolve_cache_url(cache_url.as_deref(), &dir)?;
    let client = cachet_cli::http_client()?;
    let config = cachet_cli::login::fetch_public_config(&client, &url).await?;
    let Some(token) = config::read_token(&dir, &config.host)? else {
        println!("cachet: no stored login for {}", config.host);
        return Ok(());
    };
    match cachet_cli::login::revoke_read_token(&client, &url, &token).await {
        Ok(()) => println!("cachet: {} revoked this machine's credential", config.host),
        // why: the local copy goes either way. A deployment that cannot
        // be reached must not leave a credential sitting on the disk of
        // someone who asked to be logged out, and the credential expires
        // on its own regardless.
        Err(failure) => {
            eprintln!(
                "cachet: could not reach {} to revoke: {failure}",
                config.host
            );
            eprintln!("  the local copy is removed anyway; it expires on its own.");
        }
    }
    config::forget_login(&dir, &config.host)?;
    println!("cachet: run `cachet setup` to drop it from this machine's nix");
    Ok(())
}

/// The setup flow: token from the state directory, config from the
/// deployment, files through the privileged helpers, daemon restarted.
async fn setup(
    vars: Vec<(String, String)>,
    cache_url: Option<String>,
) -> Result<(), cachet_cli::CliError> {
    let dir = config::state_dir(&vars)?;
    let url = config::resolve_cache_url(cache_url.as_deref(), &dir)?;
    let client = cachet_cli::http_client()?;
    let config = cachet_cli::login::fetch_public_config(&client, &url).await?;
    let token = config::read_token(&dir, &config.host)?.ok_or_else(|| {
        cachet_cli::CliError(format!(
            "no stored login for {}: run `cachet login --cache-url {url}` first",
            config.host
        ))
    })?;
    let paths = resolve_setup_paths(&vars);
    let run = privileged_runner(&vars);
    let install = installer(&run);
    // The daemon's trusted-users must name the invoking account; USER is
    // its spelling on every shell cachet supports, and the env-fallback
    // line names the failure loudly rather than wiring a wrong guess.
    let login = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map_err(|_| {
            cachet_cli::CliError(
                "USER and LOGNAME are both unset: cannot name the account for the daemon's trusted-users"
                    .to_string(),
            )
        })?;
    let report = cachet_cli::setup::run_setup(
        &paths,
        &cachet_cli::setup::SetupInput {
            cache_url: url,
            public_key: config.public_key,
            token,
            login,
        },
        &run,
        &install,
    )?;
    print_setup_report(&config.host, &report);
    Ok(())
}

/// The privileged command runner: argv with the sudo prefix the host's
/// CACHET_SUDO setting asks for (empty means already-root, as in tests).
fn privileged_runner(
    vars: &[(String, String)],
) -> impl Fn(&[&str]) -> Result<(), String> + Send + Sync {
    let sudo = vars
        .iter()
        .find(|(key, _)| key == "CACHET_SUDO")
        .map_or_else(|| "sudo".to_string(), |(_, v)| v.clone());
    move |argv: &[&str]| -> Result<(), String> {
        let invocation: Vec<String> = if sudo.is_empty() {
            argv.iter().map(ToString::to_string).collect()
        } else {
            std::iter::once(sudo.clone())
                .chain(argv.iter().map(ToString::to_string))
                .collect()
        };
        // why: stdin inherits (sudo's password prompt lives on the tty),
        // but stdout/stderr are captured — the reload dance's probing
        // attempts fail routinely (no systemctl on macOS) and their
        // complaints read as cachet's own failure if inherited. The
        // error string keeps the detail for the surfaces that report it.
        let output = std::process::Command::new(&invocation[0])
            .args(&invocation[1..])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|failure| format!("could not run {}: {failure}", invocation[0]))?;
        let status = output.status;
        if status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        if detail.is_empty() {
            Err(format!("{} exited {status}", invocation.join(" ")))
        } else {
            Err(format!(
                "{} exited {status}: {detail}",
                invocation.join(" ")
            ))
        }
    }
}

/// The system-file installer: compose fully at a scratch path, then
/// place through the privilege runner; a half-written system config is
/// worse than a stale one.
fn installer<'r>(
    run: &'r cachet_cli::setup::Privileged<'r>,
) -> impl Fn(&std::path::Path, &str, u32) -> Result<(), String> + 'r {
    move |path: &std::path::Path, contents: &str, mode: u32| -> Result<(), String> {
        let tmp = std::env::temp_dir().join(format!(
            "cachet-setup-{}-{}",
            std::process::id(),
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        std::fs::write(&tmp, contents)
            .map_err(|failure| format!("{}: {failure}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
                .map_err(|failure| format!("chmod {}: {failure}", tmp.display()))?;
        }
        if let Some(parent) = path.parent() {
            run(&["mkdir", "-p", &parent.to_string_lossy()])?;
        }
        run(&["cp", &tmp.to_string_lossy(), &path.to_string_lossy()])?;
        run(&["chmod", &format!("{mode:o}"), &path.to_string_lossy()])?;
        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }
}

/// The closing report: every write, then how the daemon restart went.
fn print_setup_report(host: &str, report: &cachet_cli::setup::SetupReport) {
    // Name the install. The two are wired differently (Determinate reads
    // the netrc it synthesizes from additionalNetrcSources, plain nix
    // reads netrc-file), so a reader chasing a 401 needs to know which
    // machine they are on before any of the paths below mean anything.
    if report.determinate {
        println!("cachet: Determinate Nix detected; wiring it the way it expects");
    } else {
        println!("cachet: nix detected");
    }
    let mut wrote = report.wrote.iter();
    if let Some(first) = wrote.next() {
        println!("cachet: {host} configured in {}", first.display());
    }
    for path in wrote {
        println!("cachet: wrote {}", path.display());
    }
    match &report.reload {
        cachet_cli::setup::ReloadOutcome::Systemd => {
            println!(
                "cachet: nix-daemon restarted with systemctl, so the new configuration is live"
            );
        }
        cachet_cli::setup::ReloadOutcome::Launchctl(label) => {
            println!("cachet: nix-daemon restarted ({label}), so the new configuration is live");
        }
        cachet_cli::setup::ReloadOutcome::DeterminateInit => {
            println!("cachet: determinate-nixd re-initialized, so the new configuration is live");
        }
        cachet_cli::setup::ReloadOutcome::Failed => {
            eprintln!(
                "cachet: could NOT restart nix-daemon, so the settings above are not in effect yet."
            );
            eprintln!("  Substitution will keep returning 401 until it reloads. Restart it with:");
            eprintln!(
                "    sudo launchctl kickstart -k system/systems.determinate.nix-daemon   # macOS, Determinate"
            );
            eprintln!(
                "    sudo launchctl kickstart -k system/org.nixos.nix-daemon              # macOS, upstream"
            );
            eprintln!(
                "    sudo systemctl restart nix-daemon                                    # linux"
            );
        }
    }
}

/// The path defaults and their environment overrides, plus the
/// Determinate detection: the explicit answer wins, then the PATH probe.
fn resolve_setup_paths(vars: &[(String, String)]) -> cachet_cli::setup::SetupPaths {
    let value = |name: &str, default: &str| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    let determinate = match value("CACHET_DETERMINATE", "").as_str() {
        "1" => true,
        "0" => false,
        _ => on_path("determinate-nixd", &value("PATH", "/usr/bin:/bin")),
    };
    cachet_cli::setup::SetupPaths {
        netrc: value("CACHET_NETRC_PATH", "/etc/nix/netrc").into(),
        nix_custom_conf: value("CACHET_NIX_CUSTOM_CONF_PATH", "/etc/nix/nix.custom.conf").into(),
        determinate_config: value(
            "CACHET_DETERMINATE_CONFIG_PATH",
            "/etc/determinate/config.json",
        )
        .into(),
        launch_daemons: value("CACHET_LAUNCH_DAEMONS_DIR", "/Library/LaunchDaemons").into(),
        determinate,
    }
}

/// Is `verb` runnable from this PATH?
fn on_path(verb: &str, path_var: &str) -> bool {
    path_var
        .split(':')
        .map(std::path::PathBuf::from)
        .any(|dir| dir.join(verb).is_file())
}

/// The probe run, printing every line; the exit code answers the
/// aggregate.
async fn doctor(vars: Vec<(String, String)>, cache_url: Option<String>) -> ExitCode {
    let outcome = async {
        let dir = config::state_dir(&vars)?;
        let url = config::resolve_cache_url(cache_url.as_deref(), &dir)?;
        let host = config::host_of(&url)?;
        let client = cachet_cli::http_client()?;
        let token = config::read_token(&dir, &host)?;
        Ok::<_, cachet_cli::CliError>((url, client, token))
    }
    .await;
    match outcome {
        Ok((url, client, token)) => {
            let probes = cachet_cli::doctor::run_doctor(
                &client,
                &url,
                token.as_deref(),
                cachet_cli::login::GITHUB_API_BASE,
            )
            .await;
            let (lines, all_ok) = cachet_cli::doctor::render(&probes);
            for line in lines {
                println!("{line}");
            }
            if all_ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(failure) => {
            eprintln!("cachet: {failure}");
            ExitCode::FAILURE
        }
    }
}

/// keygen: print, or write the pair under --out-dir.
fn keygen(name: &str, out_dir: Option<&std::path::Path>) -> Result<(), cachet_cli::CliError> {
    let (secret, public) = cachet_cli::keygen::generate(name)
        .map_err(|failure| cachet_cli::CliError(failure.to_string()))?;
    if let Some(dir) = out_dir {
        {
            std::fs::create_dir_all(dir).map_err(|failure| {
                cachet_cli::CliError(format!("could not create {}: {failure}", dir.display()))
            })?;
            let secret_path = dir.join("cachet-key.secret");
            let public_path = dir.join("cachet-key.public");
            std::fs::write(&secret_path, format!("{secret}\n")).map_err(|failure| {
                cachet_cli::CliError(format!(
                    "could not write {}: {failure}",
                    secret_path.display()
                ))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|failure| {
                        cachet_cli::CliError(format!(
                            "could not chmod {}: {failure}",
                            secret_path.display()
                        ))
                    })?;
            }
            std::fs::write(&public_path, format!("{public}\n")).map_err(|failure| {
                cachet_cli::CliError(format!(
                    "could not write {}: {failure}",
                    public_path.display()
                ))
            })?;
            println!(
                "cachet: secret key written to {} (keep it there or in a secret store)",
                secret_path.display()
            );
            println!("cachet: public key written to {}", public_path.display());
            println!("cachet: clients trust it as: {public}");
        }
    } else {
        println!("cachet: the secret key (store it as the deployment's signing secret):");
        println!("{secret}");
        println!("cachet: the public key (clients trust it):");
        println!("{public}");
    }
    Ok(())
}
