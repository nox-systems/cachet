//! Where the CLI keeps its state: one directory holding one token per
//! deployment and the URL of the deployment last logged into, so later
//! commands can answer "which cache" without a flag.

use std::path::{Path, PathBuf};

use crate::CliError;

/// Resolve the state directory: the explicit override, then XDG, then
/// the home fallback. The override exists so tests and unusual layouts
/// stay first-class.
///
/// # Errors
///
/// [`CliError`] when no home directory is discoverable.
pub fn state_dir(vars: &[(String, String)]) -> Result<PathBuf, CliError> {
    let value = |name: &str| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    if let Some(explicit) = value("CACHET_CONFIG_DIR") {
        return Ok(PathBuf::from(explicit));
    }
    if let Some(xdg) = value("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("cachet"));
    }
    value("HOME")
        .map(|home| PathBuf::from(home).join(".config").join("cachet"))
        .ok_or_else(|| CliError("no home directory: set CACHET_CONFIG_DIR explicitly".to_string()))
}

/// The token file for one deployment, keyed by host so several caches
/// can share the directory.
fn token_path(dir: &Path, host: &str) -> PathBuf {
    dir.join("tokens").join(format!("{host}.token"))
}

/// The file remembering which deployment was logged into most recently.
fn default_url_path(dir: &Path) -> PathBuf {
    dir.join("default-url")
}

/// Persist a login: the token at 0600 under its host, and the base URL
/// as the new default. Re-login overwrites both, so rotation is just
/// logging in again.
///
/// # Errors
///
/// [`CliError`] on any write or permission failure.
pub fn store_login(dir: &Path, host: &str, base_url: &str, token: &str) -> Result<(), CliError> {
    let tokens = dir.join("tokens");
    std::fs::create_dir_all(&tokens)
        .map_err(|failure| CliError(format!("could not create {}: {failure}", tokens.display())))?;
    let path = token_path(dir, host);
    std::fs::write(&path, format!("{token}\n"))
        .map_err(|failure| CliError(format!("could not write {}: {failure}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |failure| CliError(format!("could not chmod {}: {failure}", path.display())),
        )?;
    }
    std::fs::write(default_url_path(dir), format!("{base_url}\n")).map_err(|failure| {
        CliError(format!(
            "could not write {}: {failure}",
            default_url_path(dir).display()
        ))
    })?;
    Ok(())
}

/// The stored token for a deployment, trimmed. Absence is an answer, not
/// a failure: `Ok(None)` means login has not happened here.
///
/// # Errors
///
/// [`CliError`] on a read failure other than absence.
/// Forget one deployment's stored credential. An absent file is the
/// outcome the caller asked for, not a failure.
///
/// # Errors
///
/// [`CliError`] when the file exists and cannot be removed.
pub fn forget_login(dir: &Path, host: &str) -> Result<(), CliError> {
    let path = token_path(dir, host);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(failure) => Err(CliError(format!(
            "could not remove {}: {failure}",
            path.display()
        ))),
    }
}

/// The stored credential for one deployment, if this machine has one.
///
/// # Errors
///
/// [`CliError`] when the file exists and cannot be read.
pub fn read_token(dir: &Path, host: &str) -> Result<Option<String>, CliError> {
    match std::fs::read_to_string(token_path(dir, host)) {
        Ok(text) => Ok(text
            .lines()
            .next()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)),
        Err(miss) if miss.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(CliError(format!(
            "could not read {}: {failure}",
            token_path(dir, host).display()
        ))),
    }
}

/// The deployment last logged into, if any.
#[must_use]
pub fn read_default_url(dir: &Path) -> Option<String> {
    std::fs::read_to_string(default_url_path(dir))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The host a base URL names, for netrc and token filenames. A URL
/// whose host cannot be told from its path is a configuration bug, so
/// this fails loudly rather than guessing.
///
/// # Errors
///
/// [`CliError`] on a shape that names no host.
pub fn host_of(base_url: &str) -> Result<String, CliError> {
    let stripped = base_url
        .trim_end_matches('/')
        .strip_prefix("https://")
        .or_else(|| base_url.trim_end_matches('/').strip_prefix("http://"))
        .ok_or_else(|| {
            CliError(format!(
                "the cache URL {base_url:?} must start with https://"
            ))
        })?;
    let host = stripped
        .split(['/', ':', '?', '#'])
        .next()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| CliError(format!("the cache URL {base_url:?} names no host")))?;
    if !host
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
    {
        return Err(CliError(format!("{host:?} is not a plausible hostname")));
    }
    Ok(host.to_string())
}

/// Resolve which deployment a command talks to: the flag wins, then the
/// stored default.
///
/// # Errors
///
/// [`CliError`] naming the fix when neither exists.
pub fn resolve_cache_url(flag: Option<&str>, dir: &Path) -> Result<String, CliError> {
    if let Some(url) = flag {
        return Ok(url.trim_end_matches('/').to_string());
    }
    read_default_url(dir).ok_or_else(|| {
        CliError("no cache URL: pass --cache-url or run `cachet login --cache-url ...`".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_grammar_matches_netrc_needs() {
        assert_eq!(
            host_of("https://cache.example.com").expect("host"),
            "cache.example.com"
        );
        assert_eq!(
            host_of("https://cache.example.com/").expect("host"),
            "cache.example.com"
        );
        assert_eq!(
            host_of("https://cache.example.com:8443").expect("host"),
            "cache.example.com"
        );
        assert!(host_of("cache.example.com").is_err(), "scheme required");
        assert!(host_of("https://").is_err());
        assert!(host_of("https://evil host/").is_err());
    }

    #[test]
    fn store_and_read_round_trip() {
        let dir = tempfile::tempdir().expect("dir");
        store_login(
            dir.path(),
            "cache.example.com",
            "https://cache.example.com",
            "gho_secret",
        )
        .expect("store");
        assert_eq!(
            read_token(dir.path(), "cache.example.com").expect("read"),
            Some("gho_secret".to_string())
        );
        assert_eq!(
            read_default_url(dir.path()),
            Some("https://cache.example.com".to_string())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(token_path(dir.path(), "cache.example.com"))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "tokens are owner-only");
        }
    }

    #[test]
    fn absence_is_an_answer() {
        let dir = tempfile::tempdir().expect("dir");
        assert_eq!(read_token(dir.path(), "no.such.host").expect("read"), None);
        assert_eq!(read_default_url(dir.path()), None);
    }
}
