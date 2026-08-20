//! The `POST /roots/{project}` request body: the store paths a project is
//! keeping alive plus the installables that produced them. Bounded before
//! parsing, and every entry validated, because this body becomes the
//! collector's root set and anything in it must already be grammar-clean.

use crate::constants::{ROOTS_BODY_BYTES_MAX, ROOTS_PATHS_MAX};
use crate::error::{ClientError, Result};
use crate::keys::parse_store_path;

/// A validated roots payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootsPayload {
    /// Full nix store paths, each grammar-validated.
    pub store_paths: Vec<String>,
    /// Flake installables the job built. Never begins with `-`: an
    /// installable is passed to `nix build` as an argv entry by the
    /// deployment's verification tooling, and a dash-prefixed entry would
    /// read as a command-line flag rather than a flake reference.
    pub installables: Vec<String>,
}

/// Parse a roots payload.
///
/// # Errors
///
/// [`ClientError::BodyTooLarge`] over the byte cap;
/// [`ClientError::MalformedRoots`] for anything else.
pub fn parse_roots_payload(text: &str) -> Result<RootsPayload> {
    if u64::try_from(text.len()).expect("len fits") > ROOTS_BODY_BYTES_MAX {
        return Err(ClientError::BodyTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| ClientError::MalformedRoots)?;
    let raw = value.as_object().ok_or(ClientError::MalformedRoots)?;

    let store_paths = string_list(raw.get("storePaths"), "storePaths")?;
    let installables = string_list(raw.get("installables"), "installables")?;

    for installable in &installables {
        if installable.starts_with('-') {
            return Err(ClientError::MalformedRoots);
        }
    }
    for path in &store_paths {
        parse_store_path(path).map_err(|_| ClientError::MalformedRoots)?;
    }
    Ok(RootsPayload {
        store_paths,
        installables,
    })
}

/// Read one array-of-strings field, capped and element-validated.
fn string_list(raw: Option<&serde_json::Value>, _field: &'static str) -> Result<Vec<String>> {
    let Some(array) = raw.and_then(serde_json::Value::as_array) else {
        return Err(ClientError::MalformedRoots);
    };
    if array.len() > ROOTS_PATHS_MAX {
        return Err(ClientError::MalformedRoots);
    }
    let mut out = Vec::with_capacity(array.len());
    for entry in array {
        let Some(text) = entry.as_str() else {
            return Err(ClientError::MalformedRoots);
        };
        out.push(text.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2";

    #[test]
    fn the_happy_document() {
        let body = serde_json::json!({
            "storePaths": [PATH],
            "installables": [".#devShells.aarch64-darwin.default"],
        })
        .to_string();
        let payload = parse_roots_payload(&body).expect("the contract parses");
        assert_eq!(payload.store_paths, [PATH]);
    }

    #[test]
    fn hostile_entries_are_refused() {
        for body in [
            r"{}",
            r#"{"storePaths": []}"#,
            &serde_json::json!({"storePaths": ["/etc/passwd-name"]}).to_string(),
            &serde_json::json!({"storePaths": [], "installables": ["--option substitute true"]})
                .to_string(),
            &serde_json::json!({"storePaths": [PATH, PATH], "installables": null}).to_string(),
        ] {
            assert!(parse_roots_payload(body).is_err(), "{body} refused");
        }
    }

    #[test]
    fn overlong_bodies_are_refused_by_size() {
        let huge = format!("{{\"storePaths\": [\"{}\"]}}", PATH.repeat(20_000));
        assert_eq!(parse_roots_payload(&huge), Err(ClientError::BodyTooLarge));
    }
}
