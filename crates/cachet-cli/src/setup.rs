//! The setup command's half: land the read token where the nix daemon
//! can see it (the daemon's netrc, not the user's), point the config at
//! the cache and its signing key, and restart the daemon so the new
//! configuration is live. The merge semantics port the previous login
//! script's exactly: block-aware netrc replacement so no orphaned
//! credential lines survive, word-merging key-value config so unrelated
//! lines pass through untouched, and the Determinate divergence where
//! the netrc reaches the daemon by registration instead of by
//! `netrc-file`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::CliError;

/// Everything the edits need, resolved once by the caller: the values
/// that go in, and the places they go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupInput {
    /// The cache's base URL; its host names the netrc machine and its
    /// whole URL becomes the substituter.
    pub cache_url: String,
    /// The deployment's public key in nix's `name:base64` form.
    pub public_key: String,
    /// The read token `cachet login` stored.
    pub token: String,
    /// The invoking account's login. The daemon's `trusted-users` must
    /// name it: nix treats `extra-trusted-public-keys` as a restricted
    /// setting, so an untrusted caller's key line is parsed and silently
    /// dropped, and no cachet-signed path ever substitutes for them.
    pub login: String,
}

/// The files this command owns, absolute. The paths are fields rather
/// than constants so tests write into sandboxes and operators with
/// unusual layouts can point elsewhere through the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPaths {
    /// The daemon's netrc (written 0600).
    pub netrc: PathBuf,
    /// The included user config (written 0644).
    pub nix_custom_conf: PathBuf,
    /// Determinate's config, whose `additionalNetrcSources` registers the
    /// netrc when running under determinate-nixd.
    pub determinate_config: PathBuf,
    /// The directory macOS keeps daemon plists in; the reload ladder
    /// discovers nix-daemon labels here.
    pub launch_daemons: PathBuf,
    /// Whether the host runs Determinate Nix; the daemon reload order and
    /// the netrc-file decision both read it.
    pub determinate: bool,
}

/// Rewrite a netrc, replacing every block that names `host` and leaving
/// all other blocks byte-verbatim. A block spans from its `machine` or
/// `default` keyword to the next one, across as many lines as it
/// occupies: dropping whole blocks is what keeps orphaned `password`
/// lines from leaking a credential for a machine the file no longer
/// names.
#[must_use]
pub fn netrc_replace_block(existing: &str, host: &str, token: &str) -> String {
    #[derive(Debug, PartialEq)]
    enum Kind {
        Preamble,
        Machine(String),
        Default,
    }

    let mut spans: Vec<(Kind, &str)> = Vec::new();
    let mut start = 0_usize;
    let mut kind = Kind::Preamble;
    // The scanner walks whole tokens but keeps byte offsets, so kept
    // spans survive with their original spacing.
    let bytes = existing.as_bytes();
    let mut cursor = 0_usize;
    while cursor < existing.len() {
        while cursor < existing.len() && (bytes[cursor] as char).is_ascii_whitespace() {
            cursor += 1;
        }
        let token_start = cursor;
        while cursor < existing.len() && !(bytes[cursor] as char).is_ascii_whitespace() {
            cursor += 1;
        }
        let token = &existing[token_start..cursor];
        if token == "machine" || token == "default" {
            if token_start > start {
                spans.push((kind, &existing[start..token_start]));
            }
            if token == "machine" {
                while cursor < existing.len() && (bytes[cursor] as char).is_ascii_whitespace() {
                    cursor += 1;
                }
                let name_start = cursor;
                while cursor < existing.len() && !(bytes[cursor] as char).is_ascii_whitespace() {
                    cursor += 1;
                }
                kind = Kind::Machine(existing[name_start..cursor].to_string());
            } else {
                kind = Kind::Default;
            }
            start = token_start;
        }
    }
    if existing.len() > start {
        spans.push((kind, &existing[start..]));
    }

    let mut out = String::new();
    for (kind, span) in spans {
        if matches!(kind, Kind::Machine(ref name) if name == host) {
            continue;
        }
        out.push_str(span);
    }
    let trimmed = out.trim_end();
    let mut rewritten = if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    };
    let _ = writeln!(rewritten, "machine {host} login cachet password {token}");
    rewritten
}

/// The keys setup owns in the included config: every line naming one is
/// rewritten from scratch, everything else survives verbatim.
const MANAGED_KEYS: [&str; 4] = [
    "netrc-file",
    "extra-substituters",
    "extra-trusted-public-keys",
    "trusted-users",
];

/// Merge the managed keys into the included config. The surviving words
/// keep their order, the wanted words join them once (a rerun converges
/// to a stable file), and the managed lines emit in a fixed order at
/// the end: exactly one line per key, matching nix's last-line-wins
/// reading.
#[must_use]
pub fn merge_custom_conf(
    existing: &str,
    netrc_path: Option<&str>,
    substituter: &str,
    key: &str,
    login: &str,
) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut existing_substituters: Vec<String> = Vec::new();
    let mut existing_keys: Vec<String> = Vec::new();
    let mut existing_users: Vec<String> = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let managed = MANAGED_KEYS.iter().find_map(|name| {
            trimmed
                .strip_prefix(name)
                .filter(|rest| rest.trim_start().starts_with('='))
                .map(|rest| (*name, rest.trim_start()[1..].trim()))
        });
        match managed {
            Some(("extra-substituters", value)) => {
                existing_substituters = value.split_whitespace().map(str::to_string).collect();
            }
            Some(("extra-trusted-public-keys", value)) => {
                existing_keys = value.split_whitespace().map(str::to_string).collect();
            }
            Some(("trusted-users", value)) => {
                existing_users = value.split_whitespace().map(str::to_string).collect();
            }
            Some(("netrc-file", _)) => {}
            Some(_) => unreachable!("MANAGED_KEYS has four entries"),
            None => kept.push(line),
        }
    }

    let mut substituters = existing_substituters;
    if !substituters.iter().any(|word| word == substituter) {
        substituters.push(substituter.to_string());
    }
    let mut keys = existing_keys;
    if !key.is_empty() && !keys.iter().any(|word| word == key) {
        keys.push(key.to_string());
    }
    // why: root first because daemons already name it, the invoking login
    // because restricted key settings only reach the evaluator for users
    // this list trusts; without the login every key line above is inert.
    let mut users = existing_users;
    for wanted in ["root", login] {
        if !users.iter().any(|word| word == wanted) {
            users.push(wanted.to_string());
        }
    }

    let mut out = String::new();
    for line in kept {
        let _ = writeln!(out, "{line}");
    }
    if let Some(path) = netrc_path {
        let _ = writeln!(out, "netrc-file = {path}");
    }
    let _ = writeln!(out, "extra-substituters = {}", substituters.join(" "));
    if !keys.is_empty() {
        let _ = writeln!(out, "extra-trusted-public-keys = {}", keys.join(" "));
    }
    let _ = writeln!(out, "trusted-users = {}", users.join(" "));
    out
}

/// Register the daemon netrc under Determinate's
/// `additionalNetrcSources`, preserving whatever else the file holds.
/// determinate-nixd rewrites the daemon's `netrc-file` after the config
/// include runs, so registration is the only durable route on those
/// hosts.
///
/// # Errors
///
/// [`CliError`] when an existing file is not JSON.
pub fn merge_determinate_config(existing: Option<&str>) -> Result<String, CliError> {
    const SOURCE: &str = "/etc/nix/netrc";
    let mut root: serde_json::Value = match existing {
        None => serde_json::json!({}),
        Some(text) => serde_json::from_str(text).map_err(|failure| {
            CliError(format!(
                "the Determinate config exists but does not parse as JSON: {failure}"
            ))
        })?,
    };
    if !root.is_object() {
        return Err(CliError(
            "the Determinate config is not a JSON object".to_string(),
        ));
    }
    let authentication = root
        .as_object_mut()
        .expect("checked above")
        .entry("authentication")
        .or_insert_with(|| serde_json::json!({}));
    if !authentication.is_object() {
        return Err(CliError(
            "the Determinate config's `authentication` is not an object".to_string(),
        ));
    }
    let sources = authentication
        .as_object_mut()
        .expect("checked above")
        .entry("additionalNetrcSources")
        .or_insert_with(|| serde_json::json!([]));
    let list = sources.as_array_mut().ok_or_else(|| {
        CliError("the Determinate config's `additionalNetrcSources` is not a list".to_string())
    })?;
    if !list.iter().any(|entry| entry.as_str() == Some(SOURCE)) {
        list.push(serde_json::Value::String(SOURCE.to_string()));
    }
    let mut body = serde_json::to_string_pretty(&root).expect("a JSON object serializes");
    body.push('\n');
    Ok(body)
}

/// How the daemon restart went, for the closing report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// `systemctl restart nix-daemon` ran clean.
    Systemd,
    /// `launchctl kickstart -k system/<label>` ran clean.
    Launchctl(String),
    /// `determinate-nixd init` ran clean.
    DeterminateInit,
    /// Nothing worked; the operator restart lines already printed.
    Failed,
}

/// One privileged command, injected so tests script it and main runs
/// argv. Arguments arrive without the sudo prefix; the runner applies
/// whatever privilege escalation the host needs.
pub type Privileged<'a> = dyn Fn(&[&str]) -> Result<(), String> + Send + Sync + 'a;

/// The restart dance, ordered: systemd first (linux), then each macOS
/// daemon plist by label, then determinate-nixd's own initializer.
/// A cache whose daemon never reloaded answers 401 on every read, so
/// each attempt is a real command, not a config note.
#[must_use = "the outcome decides what the report says"]
pub fn reload_daemon(paths: &SetupPaths, run: &Privileged<'_>) -> ReloadOutcome {
    if run(&["systemctl", "restart", "nix-daemon"]).is_ok() {
        return ReloadOutcome::Systemd;
    }
    let mut labels: Vec<String> = std::fs::read_dir(&paths.launch_daemons)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    let is_plist = path.extension().is_some_and(|ext| ext == "plist");
                    let name = entry.file_name().to_string_lossy().into_owned();
                    (is_plist && name.contains("nix-daemon"))
                        .then(|| name.trim_end_matches(".plist").to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    labels.sort();
    for label in labels {
        if run(&["launchctl", "kickstart", "-k", &format!("system/{label}")]).is_ok() {
            return ReloadOutcome::Launchctl(label);
        }
    }
    if run(&["determinate-nixd", "init"]).is_ok() {
        return ReloadOutcome::DeterminateInit;
    }
    ReloadOutcome::Failed
}

/// What setup wrote, for the closing report.
#[derive(Debug)]
pub struct SetupReport {
    /// Each file the command (re)wrote.
    pub wrote: Vec<PathBuf>,
    /// How the daemon restart went.
    pub reload: ReloadOutcome,
    /// Whether the host runs Determinate Nix. Reported because the two
    /// installs are wired differently and a reader deserves to know
    /// which one this machine got.
    pub determinate: bool,
}

/// The full setup: rewrite the three files, then restart the daemon.
/// Writes go through one helper so each lands complete (temp file,
/// chmod, then the privileged copy), never half-written.
///
/// # Errors
///
/// [`CliError`] on the first write or parse failure; the daemon reload
/// is best-effort because its failure mode is diagnosed, not fatal.
pub fn run_setup(
    paths: &SetupPaths,
    input: &SetupInput,
    run: &Privileged<'_>,
    install: &dyn Fn(&Path, &str, u32) -> Result<(), String>,
) -> Result<SetupReport, CliError> {
    let host = crate::config::host_of(&input.cache_url)?;
    let mut wrote = Vec::new();

    let existing_netrc = std::fs::read_to_string(&paths.netrc).unwrap_or_default();
    let netrc = netrc_replace_block(&existing_netrc, &host, &input.token);
    install(&paths.netrc, &netrc, 0o600).map_err(|failure| {
        CliError(format!(
            "could not write {}: {failure}",
            paths.netrc.display()
        ))
    })?;
    wrote.push(paths.netrc.clone());

    if paths.determinate {
        let existing = std::fs::read_to_string(&paths.determinate_config).ok();
        let config = merge_determinate_config(existing.as_deref())?;
        install(&paths.determinate_config, &config, 0o644).map_err(|failure| {
            CliError(format!(
                "could not write {}: {failure}",
                paths.determinate_config.display()
            ))
        })?;
        wrote.push(paths.determinate_config.clone());
    }

    let existing_conf = std::fs::read_to_string(&paths.nix_custom_conf).unwrap_or_default();
    let netrc_line = if paths.determinate {
        None
    } else {
        Some(paths.netrc.to_string_lossy().into_owned())
    };
    let conf = merge_custom_conf(
        &existing_conf,
        netrc_line.as_deref(),
        &input.cache_url,
        &input.public_key,
        &input.login,
    );
    install(&paths.nix_custom_conf, &conf, 0o644).map_err(|failure| {
        CliError(format!(
            "could not write {}: {failure}",
            paths.nix_custom_conf.display()
        ))
    })?;
    wrote.push(paths.nix_custom_conf.clone());

    let reload = reload_daemon(paths, run);
    Ok(SetupReport {
        wrote,
        reload,
        determinate: paths.determinate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netrc_replacement_is_block_aware() {
        let existing = concat!(
            "machine other.example.com login a password b\n",
            "machine cache.example.com\n    login cachet\n    password old-token\n",
            "default login anon password none\n",
        );
        let rewritten = netrc_replace_block(existing, "cache.example.com", "new-token");
        assert_eq!(
            rewritten,
            concat!(
                "machine other.example.com login a password b\n",
                "default login anon password none\n",
                "machine cache.example.com login cachet password new-token\n",
            ),
            "other blocks survive verbatim; the host block is gone whole"
        );
    }

    #[test]
    fn netrc_rewrite_is_idempotent() {
        let once = netrc_replace_block("", "cache.example.com", "tok");
        let twice = netrc_replace_block(&once, "cache.example.com", "tok");
        assert_eq!(once, twice);
    }

    #[test]
    fn netrc_handles_entries_sharing_a_line() {
        let existing = "machine a.example.com login x password y machine cache.example.com login cachet password old\n";
        let rewritten = netrc_replace_block(existing, "cache.example.com", "new");
        assert_eq!(
            rewritten,
            "machine a.example.com login x password y\nmachine cache.example.com login cachet password new\n"
        );
    }

    #[test]
    fn custom_conf_merges_words_and_drops_strays() {
        let existing = concat!(
            "cores = 8\n",
            "\n",
            "extra-substituters = https://cache.nixos.org\n",
            "extra-substituters = https://cache.nixos.org https://old.invalid\n",
            "netrc-file = /somewhere/else\n",
            "extra-trusted-public-keys = cache.nixos.org-1:aaaa\n",
        );
        let merged = merge_custom_conf(
            existing,
            Some("/etc/nix/netrc"),
            "https://cache.example.com",
            "cache.example.com-1:bbbb",
            "tester",
        );
        assert_eq!(
            merged,
            concat!(
                "cores = 8\n",
                "netrc-file = /etc/nix/netrc\n",
                "extra-substituters = https://cache.nixos.org https://old.invalid https://cache.example.com\n",
                "extra-trusted-public-keys = cache.nixos.org-1:aaaa cache.example.com-1:bbbb\n",
                "trusted-users = root tester\n",
            ),
            "last line wins, words merge once, one line per key in fixed order"
        );
        let again = merge_custom_conf(
            &merged,
            Some("/etc/nix/netrc"),
            "https://cache.example.com",
            "cache.example.com-1:bbbb",
            "tester",
        );
        assert_eq!(merged, again, "a rerun changes nothing");
    }

    #[test]
    fn custom_conf_under_determinate_omits_netrc_file() {
        let merged = merge_custom_conf("", None, "https://cache.example.com", "k-1:v", "tester");
        assert!(!merged.contains("netrc-file"));
        assert!(merged.contains("extra-substituters = https://cache.example.com\n"));
        assert!(merged.contains("extra-trusted-public-keys = k-1:v\n"));
        assert!(
            merged.contains("trusted-users = root tester\n"),
            "an untrusted caller's restricted key settings drop silently"
        );
    }

    #[test]
    fn existing_trusted_users_survive_and_root_stays_first_class() {
        let merged = merge_custom_conf(
            "trusted-users = @admin existing\n",
            None,
            "https://cache.example.com",
            "k-1:v",
            "tester",
        );
        assert_eq!(
            merged,
            concat!(
                "extra-substituters = https://cache.example.com\n",
                "extra-trusted-public-keys = k-1:v\n",
                "trusted-users = @admin existing root tester\n",
            ),
            "operator-listed users keep their order; root and the login join once"
        );
    }

    #[test]
    fn determinate_config_registers_once() {
        let created = merge_determinate_config(None).expect("created");
        let parsed: serde_json::Value = serde_json::from_str(&created).expect("parses");
        assert_eq!(
            parsed["authentication"]["additionalNetrcSources"],
            serde_json::json!(["/etc/nix/netrc"])
        );
        let again = merge_determinate_config(Some(&created)).expect("merge");
        let parsed: serde_json::Value = serde_json::from_str(&again).expect("parses");
        assert_eq!(
            parsed["authentication"]["additionalNetrcSources"]
                .as_array()
                .expect("list")
                .len(),
            1,
            "registering twice does not duplicate"
        );
    }

    #[test]
    fn determinate_config_preserves_other_sections() {
        let existing = r#"{"builder":{"speedFactor":2},"authentication":{"additionalNetrcSources":["/run/secrets/netrc"]}}"#;
        let merged = merge_determinate_config(Some(existing)).expect("merge");
        let parsed: serde_json::Value = serde_json::from_str(&merged).expect("parses");
        assert_eq!(parsed["builder"]["speedFactor"], 2);
        assert_eq!(
            parsed["authentication"]["additionalNetrcSources"],
            serde_json::json!(["/run/secrets/netrc", "/etc/nix/netrc"])
        );
    }

    #[test]
    fn daemon_reload_walks_the_ladder() {
        let dir = tempfile::tempdir().expect("dir");
        let daemons = dir.path().join("etc").join("LaunchDaemons");
        std::fs::create_dir_all(&daemons).expect("dir");
        std::fs::write(daemons.join("systems.determinate.nix-daemon.plist"), "").expect("plist");
        std::fs::write(daemons.join("unrelated.plist"), "").expect("plist");
        let paths = SetupPaths {
            netrc: dir.path().join("etc/nix/netrc"),
            nix_custom_conf: dir.path().join("etc/nix/nix.custom.conf"),
            determinate_config: dir.path().join("etc/determinate/config.json"),
            launch_daemons: daemons.clone(),
            determinate: true,
        };
        // systemctl refuses; launchctl answers.
        let ran: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());
        let run = |argv: &[&str]| -> Result<(), String> {
            ran.lock()
                .expect("log")
                .push(argv.iter().map(ToString::to_string).collect());
            match argv[0] {
                "systemctl" => Err("no systemd here".to_string()),
                "launchctl" => Ok(()),
                other => panic!("unexpected verb {other}"),
            }
        };
        let outcome = reload_daemon(&paths, &run);
        assert_eq!(
            outcome,
            ReloadOutcome::Launchctl("systems.determinate.nix-daemon".to_string())
        );
        let log = ran.lock().expect("log");
        assert_eq!(log[0], vec!["systemctl", "restart", "nix-daemon"]);
        assert_eq!(
            log[1],
            vec![
                "launchctl",
                "kickstart",
                "-k",
                "system/systems.determinate.nix-daemon"
            ]
        );
    }
}
