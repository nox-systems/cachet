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
        invoke(vec![
            "nix".to_string(),
            "path-info".to_string(),
            installable.to_string(),
        ])
        .await
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
