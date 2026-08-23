//! The production adapters: reqwest against the wire, tokio against nix
//! and the filesystem. Thin by design: the pipeline's shapes ARE the
//! contract, so an adapter maps calls to libraries and nothing more.

use std::path::Path;

use crate::adapters::{Commands, Http, WireAnswer};
use crate::error::PushError;

/// The reqwest-backed wire.
pub struct ReqwestHttp {
    client: reqwest::Client,
}

impl ReqwestHttp {
    /// Build one client per pipeline.
    ///
    /// # Errors
    ///
    /// [`PushError::Detail`] when the TLS stack cannot initialize.
    pub fn new() -> Result<Self, PushError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("cachet/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|failure| PushError::Detail {
                message: format!("could not build the HTTP client: {failure}"),
            })?;
        Ok(Self { client })
    }

    /// One request with its small fixed shape, answered as status plus
    /// body.
    async fn round_trip(
        &self,
        request: reqwest::RequestBuilder,
        label: &str,
    ) -> Result<WireAnswer, PushError> {
        let response = request.send().await.map_err(|failure| PushError::Detail {
            message: format!("{label}: {failure}"),
        })?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("{label} answer: {failure}"),
            })?
            .to_vec();
        Ok(WireAnswer { status, body })
    }
}

impl Http for ReqwestHttp {
    async fn head(&self, url: &str, bearer: Option<&str>) -> Result<u16, PushError> {
        let mut request = self.client.head(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let status = request
            .send()
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("HEAD {url}: {failure}"),
            })?
            .status()
            .as_u16();
        Ok(status)
    }

    async fn get(&self, url: &str, bearer: Option<&str>) -> Result<WireAnswer, PushError> {
        let mut request = self.client.get(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        self.round_trip(request, &format!("GET {url}")).await
    }

    async fn put(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<WireAnswer, PushError> {
        // why: the worker's 411 guard reads the header, never the stream.
        let mut request = self
            .client
            .put(url)
            .bearer_auth(bearer)
            .header("content-length", body.len().to_string())
            .body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        self.round_trip(request, &format!("PUT {url}")).await
    }

    async fn post(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<WireAnswer, PushError> {
        let mut request = self
            .client
            .post(url)
            .bearer_auth(bearer)
            .header("content-length", body.len().to_string())
            .body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        self.round_trip(request, &format!("POST {url}")).await
    }

    async fn delete(&self, url: &str, bearer: &str) -> Result<u16, PushError> {
        let status = self
            .client
            .delete(url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("DELETE {url}: {failure}"),
            })?
            .status()
            .as_u16();
        Ok(status)
    }
}

/// The tokio-backed nix adapter: argv in, stdout out, stderr in the
/// failure text.
pub struct TokioCommands;

/// One invocation, the previous pipeline's envelope: the answer is
/// stdout; the failure names the argv and its complaint.
async fn invoke(argv: Vec<String>) -> Result<String, PushError> {
    let printed = argv.join(" ");
    let output = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .await
        .map_err(|failure| PushError::CommandFailed {
            argv: printed.clone(),
            message: failure.to_string(),
        })?;
    if !output.status.success() {
        return Err(PushError::CommandFailed {
            argv: printed,
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

impl Commands for TokioCommands {
    async fn path_info_all(&self) -> Result<String, PushError> {
        invoke(vec![
            "nix".to_string(),
            "path-info".to_string(),
            "--all".to_string(),
        ])
        .await
    }

    async fn path_info(&self, installable: &str) -> Result<String, PushError> {
        match invoke(vec![
            "nix".to_string(),
            "path-info".to_string(),
            installable.to_string(),
        ])
        .await
        {
            Ok(answer) => Ok(answer),
            Err(path_info_failure) => {
                // why: nix path-info insists the path exists right now; a
                // root that was never built in this store (a devShell) has
                // no answer for it. nix derivation show evaluates without
                // building, and its outputs name the paths either way.
                let shown = invoke(vec![
                    "nix".to_string(),
                    "derivation".to_string(),
                    "show".to_string(),
                    installable.to_string(),
                ])
                .await
                .map_err(|_| path_info_failure.clone())?;
                let paths = derivation_out_paths(&shown).map_err(|_| path_info_failure)?;
                Ok(paths.join("\n"))
            }
        }
    }

    async fn copy_to(&self, destination: &str, paths: &[String]) -> Result<(), PushError> {
        let mut argv: Vec<String> = ["nix", "copy", "--to", destination]
            .iter()
            .map(|part| (*part).to_string())
            .collect();
        argv.extend(paths.iter().cloned());
        invoke(argv).await.map(|_| ())
    }

    async fn read_dir(&self, dir: &Path) -> Result<Vec<(String, u64)>, PushError> {
        // why: the staging layout is two levels by nix's construction:
        // top-level narinfos plus nar/'s objects, nothing deeper.
        let mut entries = Vec::new();
        let mut read =
            tokio::fs::read_dir(dir)
                .await
                .map_err(|failure| PushError::StagingUnreadable {
                    message: format!("{}: {failure}", dir.display()),
                })?;
        while let Some(entry) =
            read.next_entry()
                .await
                .map_err(|failure| PushError::StagingUnreadable {
                    message: format!("{}: {failure}", dir.display()),
                })?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "nar" && entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                let mut inner = tokio::fs::read_dir(entry.path()).await.map_err(|failure| {
                    PushError::StagingUnreadable {
                        message: format!("{}: {failure}", entry.path().display()),
                    }
                })?;
                while let Some(object) =
                    inner
                        .next_entry()
                        .await
                        .map_err(|failure| PushError::StagingUnreadable {
                            message: format!("{}: {failure}", entry.path().display()),
                        })?
                {
                    let size = object
                        .metadata()
                        .await
                        .map_err(|failure| PushError::StagingUnreadable {
                            message: format!("{}: {failure}", object.path().display()),
                        })?
                        .len();
                    entries.push((
                        format!("nar/{}", object.file_name().to_string_lossy()),
                        size,
                    ));
                }
                continue;
            }
            if !entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let size = entry
                .metadata()
                .await
                .map_err(|failure| PushError::StagingUnreadable {
                    message: format!("{}: {failure}", entry.path().display()),
                })?
                .len();
            entries.push((name, size));
        }
        entries.sort();
        Ok(entries)
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, PushError> {
        tokio::fs::read(path)
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("{}: {failure}", path.display()),
            })
    }

    async fn read_range(&self, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, PushError> {
        use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("{}: {failure}", path.display()),
            })?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("{}: {failure}", path.display()),
            })?;
        // why: part lengths answer to the 64 MiB cap, far past 32-bit's
        // reach anyway, and the plan bounds both directions.
        let cap = usize::try_from(len).expect("part lengths fit memory");
        let mut body = vec![0_u8; cap];
        file.read_exact(&mut body)
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("{}: {failure}", path.display()),
            })?;
        Ok(body)
    }
}

/// The out paths of one `nix derivation show` answer: pure evaluation
/// already names them, built or not. Newer nix wraps the document in a
/// `derivations` object keyed by the derivation's store name and answers
/// store-relative paths; older answers are flat and absolute. Both read
/// as one set of absolute store paths.
///
/// # Errors
///
/// [`PushError::Detail`] when the answer is missing or no object at all:
/// the caller reports the original path-info failure instead.
fn derivation_out_paths(shown: &str) -> Result<Vec<String>, PushError> {
    let parsed: serde_json::Value =
        serde_json::from_str(shown).map_err(|failure| PushError::Detail {
            message: format!("derivation show did not parse: {failure}"),
        })?;
    let document = parsed.as_object().ok_or_else(|| PushError::Detail {
        message: "derivation show answered a non-object".to_string(),
    })?;
    let derivations = document
        .get("derivations")
        .and_then(serde_json::Value::as_object)
        .unwrap_or(document);
    let derivation = derivations
        .values()
        .next()
        .ok_or_else(|| PushError::Detail {
            message: "derivation show answered no derivation".to_string(),
        })?;
    let outputs = derivation
        .get("outputs")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| PushError::Detail {
            message: "derivation show answered no outputs".to_string(),
        })?;
    let mut paths: Vec<String> = outputs
        .values()
        .filter_map(|output| output.get("path").and_then(serde_json::Value::as_str))
        .map(|path| {
            if path.starts_with("/nix/store/") {
                path.to_string()
            } else {
                format!("/nix/store/{path}")
            }
        })
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapped_relative_shape_absolutizes() {
        // Determinate Nix 2.34's answer: a `derivations` wrapper, keyed
        // by store name, paths store-relative.
        let shown = r#"{
            "derivations": {
                "aaaa-nix-shell.drv": {
                    "outputs": {
                        "out": {"path": "bbbb-nix-shell"}
                    }
                }
            }
        }"#;
        assert_eq!(
            derivation_out_paths(shown).expect("the answer reads"),
            vec!["/nix/store/bbbb-nix-shell".to_string()],
        );
    }

    #[test]
    fn the_flat_absolute_shape_reads_every_output() {
        let shown = r#"{
            "/nix/store/aaaa-zstd.drv": {
                "outputs": {
                    "dev": {"path": "/nix/store/cccc-zstd-dev"},
                    "bin": {"path": "/nix/store/bbbb-zstd-bin"},
                    "out": {"path": "/nix/store/dddd-zstd"}
                }
            }
        }"#;
        assert_eq!(
            derivation_out_paths(shown).expect("the answer reads"),
            vec![
                "/nix/store/bbbb-zstd-bin".to_string(),
                "/nix/store/cccc-zstd-dev".to_string(),
                "/nix/store/dddd-zstd".to_string(),
            ],
        );
    }

    #[test]
    fn a_shapeless_answer_is_a_typed_refusal() {
        assert!(derivation_out_paths("").is_err());
        assert!(derivation_out_paths("{}").is_err());
        assert!(derivation_out_paths(r#"{"/nix/store/a.drv": {}}"#).is_err());
    }
}
