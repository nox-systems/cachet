//! The production adapters: reqwest against the wire, tokio against nix
//! and the filesystem. Thin by design: the pipeline's shapes ARE the
//! contract, so an adapter maps calls to libraries and nothing more.

use crate::adapters::{Commands, Http, UploadBody, WireAnswer};
use crate::error::PushError;
use crate::stage::{
    COMPRESSION_LEVEL, NarBody, PathFacts, SPILL_THRESHOLD_BYTES, StagedNar, parse_path_facts,
};
use tokio::io::AsyncReadExt as _;

/// How many connections the client keeps warm per host: the upload
/// window's width, so a finished request hands its connection straight to
/// the next path rather than closing it.
const UPLOAD_CONCURRENCY_HINT: usize = 16;

/// Turn a body into something reqwest can send.
///
/// Bytes already in memory go as they are. A file range is opened and
/// streamed, so a ninety-mebibyte part is read as the wire drains it
/// rather than materialized first; the previous pipeline read every part
/// into a fresh vector and then cloned it once per attempt.
async fn upload_body(body: UploadBody) -> Result<reqwest::Body, PushError> {
    match body {
        UploadBody::Bytes(bytes) => Ok(reqwest::Body::from(bytes.to_vec())),
        UploadBody::FileRange { path, offset, len } => {
            use tokio::io::AsyncSeekExt as _;
            let mut file =
                tokio::fs::File::open(&path)
                    .await
                    .map_err(|failure| PushError::Detail {
                        message: format!("{}: {failure}", path.display()),
                    })?;
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|failure| PushError::Detail {
                    message: format!("{} seek to {offset}: {failure}", path.display()),
                })?;
            let stream = tokio_util_reader_stream(file.take(len));
            Ok(reqwest::Body::wrap_stream(stream))
        }
    }
}

/// A framed reader as a byte stream, without pulling in a crate for the
/// one adapter this pipeline needs.
fn tokio_util_reader_stream<R: tokio::io::AsyncRead + Send + Unpin + 'static>(
    reader: R,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send {
    // why: 64 KiB per yield. Small enough that a cancelled upload drops
    // almost nothing, large enough that a hundred-megabyte part is a few
    // thousand yields rather than a few million.
    const CHUNK: usize = 64 * 1024;
    futures_util::stream::try_unfold(reader, |mut reader| async move {
        use tokio::io::AsyncReadExt as _;
        let mut buffer = vec![0_u8; CHUNK];
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.truncate(read);
        Ok(Some((buffer, reader)))
    })
}

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
            // why: the upload window keeps sixteen requests in flight. On
            // HTTP/1.1 that is sixteen connections and sixteen TLS
            // handshakes per wave; over HTTP/2 it is one connection
            // carrying sixteen streams, which is what the cache's edge
            // speaks anyway.
            .pool_max_idle_per_host(UPLOAD_CONCURRENCY_HINT)
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
        body: UploadBody,
        headers: &[(String, String)],
    ) -> Result<WireAnswer, PushError> {
        // why: the worker's 411 guard reads the header, never the stream.
        let length = body.len();
        let mut request = self
            .client
            .put(url)
            .bearer_auth(bearer)
            .header("content-length", length.to_string())
            .body(upload_body(body).await?);
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

    async fn path_facts(&self, paths: &[String]) -> Result<Vec<PathFacts>, PushError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // why: one invocation for the whole set. nix answers a map keyed
        // by store path, so the cost of asking about three thousand paths
        // is one process rather than three thousand.
        let mut argv = vec![
            "nix".to_string(),
            "path-info".to_string(),
            "--json".to_string(),
        ];
        argv.extend(paths.iter().cloned());
        let text = invoke(argv).await?;
        parse_path_facts(&text)
    }

    async fn stage_nar(&self, facts: &PathFacts) -> Result<StagedNar, PushError> {
        let facts = facts.clone();
        // why: compression is CPU work, and the pipeline keeps sixteen
        // paths in flight. Running it on the async runtime's own threads
        // would starve every request waiting on the wire, so each path
        // compresses on the blocking pool and the runtime keeps polling.
        tokio::task::spawn_blocking(move || stage_one(&facts))
            .await
            .map_err(|failure| PushError::Detail {
                message: format!("the staging task did not finish: {failure}"),
            })?
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

/// The sink a staged NAR's compressed bytes are written into.
///
/// It measures what passes and decides where it lands. Small paths, which
/// are nearly all of them, never leave memory. A path whose compressed
/// form grows past the threshold spills to one scratch file, so a window
/// full of large paths costs bounded memory instead of all of them at
/// once.
struct MeasuringSink {
    hasher: cachet_crypto::sha256::Sha256Stream,
    buffer: Vec<u8>,
    file: Option<tempfile::NamedTempFile>,
}

impl MeasuringSink {
    fn new() -> Self {
        Self {
            hasher: cachet_crypto::sha256::Sha256Stream::new(),
            buffer: Vec::new(),
            file: None,
        }
    }

    /// Move what is buffered onto disk and keep writing there.
    fn spill(&mut self) -> std::io::Result<()> {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&self.buffer)?;
        self.buffer = Vec::new();
        self.file = Some(file);
        Ok(())
    }

    /// Close the sink and answer what it measured and where it put things.
    fn finish(mut self) -> std::io::Result<Measured> {
        use std::io::Write as _;
        let file_size_bytes = self.hasher.byte_count();
        let file_hash_nix32 = cachet_crypto::base32::encode(&self.hasher.digest_so_far());
        let body = match self.file.take() {
            Some(mut file) => {
                file.flush()?;
                NarBody::File(std::sync::Arc::new(file.into_temp_path()))
            }
            None => NarBody::Bytes(std::sync::Arc::from(self.buffer.as_slice())),
        };
        Ok(Measured {
            file_hash_nix32,
            file_size_bytes,
            body,
        })
    }
}

impl std::io::Write for MeasuringSink {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(chunk);
        if let Some(file) = &mut self.file {
            file.write_all(chunk)?;
        } else {
            self.buffer.extend_from_slice(chunk);
            if self.buffer.len() > SPILL_THRESHOLD_BYTES {
                self.spill()?;
            }
        }
        Ok(chunk.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.file {
            return file.flush();
        }
        Ok(())
    }
}

/// What the sink measured.
struct Measured {
    file_hash_nix32: String,
    file_size_bytes: u64,
    body: NarBody,
}

/// Serialize, compress, hash, and hold one path's NAR.
///
/// Synchronous by design: it runs on the blocking pool, where a long
/// stretch of compression is what the thread is for. nix streams the NAR
/// on its stdout and the encoder consumes it as it arrives, so a path
/// never exists uncompressed anywhere but in the pipe between them.
fn stage_one(facts: &PathFacts) -> Result<StagedNar, PushError> {
    let argv = format!("nix store dump-path {}", facts.store_path);
    let failed = |message: String| PushError::CommandFailed {
        argv: argv.clone(),
        message,
    };
    let mut child = std::process::Command::new("nix")
        .args(["store", "dump-path", &facts.store_path])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|failure| failed(failure.to_string()))?;
    let mut stdout = child.stdout.take().expect("stdout was piped");

    let mut encoder = zstd::stream::write::Encoder::new(MeasuringSink::new(), COMPRESSION_LEVEL)
        .map_err(|failure| failed(format!("the compressor would not start: {failure}")))?;
    let copied = std::io::copy(&mut stdout, &mut encoder);
    // The child's status is read either way: a dump that died mid-stream
    // explains itself on stderr, and reporting the copy's own error
    // instead would name the pipe rather than the reason.
    let status = child
        .wait()
        .map_err(|failure| failed(failure.to_string()))?;
    if !status.success() {
        let mut complaint = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read as _;
            let _ = stderr.read_to_string(&mut complaint);
        }
        return Err(failed(complaint.trim().to_string()));
    }
    copied.map_err(|failure| failed(format!("the NAR stream broke: {failure}")))?;
    let measured = encoder
        .finish()
        .map_err(|failure| failed(format!("the compressor would not close: {failure}")))?
        .finish()
        .map_err(|failure| failed(format!("the staged NAR would not settle: {failure}")))?;

    Ok(StagedNar {
        facts: facts.clone(),
        file_hash_nix32: measured.file_hash_nix32,
        file_size_bytes: measured.file_size_bytes,
        body: measured.body,
    })
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
