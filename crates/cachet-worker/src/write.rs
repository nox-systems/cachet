//! Accepting objects: the single PUT, the multipart sequence, and the
//! verify-then-sign pipeline for narinfos. The guards, plan, claim checks,
//! and completion grammar are cachet-core's; the hashing, decoding, and
//! signing are cachet-crypto's; this module performs the bucket and
//! request I/O in exactly that order. Two invariants are enforced here
//! rather than trusted: a narinfo is stored only after its NAR verifies
//! byte-for-byte (NEVER-DANGLE), and the signing step accepts only the
//! verifier's output (verify-then-sign).

use cachet_core::constants::{
    COMPLETE_BODY_BYTES_MAX, NAR_DECOMPRESSED_BYTES_MAX, NAR_FACTS_BYTES_MAX, NARINFO_BYTES_MAX,
    UPLOAD_SINGLE_MAX_BYTES, UPLOADS_KEY_PREFIX,
};
use cachet_core::error::ClientError;
use cachet_core::keys::NarKey;
use cachet_core::multipart::{check_part, parse_completion_body};
use cachet_core::nar_facts::{NarFacts, facts_key};
use cachet_core::narinfo::Narinfo;
use cachet_core::types::{StorePathHash, UnixMillis};
use cachet_core::upload_record::UploadRecord;
use cachet_core::write::{
    VerifiedNar, check_narinfo_claims, nar_decode_bound, require_content_length,
};
use cachet_crypto::base32::encode as nix32_encode;
use cachet_crypto::ed25519::NixSecretKey;
use cachet_crypto::sha256::Sha256Stream;
use cachet_crypto::zstd_stream::ZstdStream;
use futures_util::{Stream, StreamExt as _};
use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use worker::{Env, FixedLengthStream, Request, Response, Result};

use crate::{error, log};

/// The header a multipart client declares the total size in.
const UPLOAD_TOTAL_BYTES_HEADER: &str = "x-cachet-upload-bytes";

/// The header a NAR write declares its decompressed size in. The decoder
/// needs a ceiling before it starts, and the uploader is the only party
/// that knows the number at that point: the narinfo carrying `NarSize`
/// arrives later, and reading it first would mean storing a NAR nobody
/// has bounded. A declaration that disagrees with the narinfo's `NarSize`
/// costs the client its narinfo, because the measured facts are what
/// verification compares against.
const NAR_DECLARED_BYTES_HEADER: &str = "x-cachet-nar-bytes";

/// 204 with no body: the answer to a write that stored what it was asked
/// to.
fn stored_response() -> Result<Response> {
    Ok(Response::empty()?.with_status(204))
}

/// Where an upload's bookkeeping record lives: a reserved prefix no
/// request path can reach.
fn upload_record_key(upload_id: &str) -> String {
    format!("{UPLOADS_KEY_PREFIX}{upload_id}")
}

/// Render a typed failure as its problem document.
fn rejected(code: ClientError) -> Result<Response> {
    error::problem_response(code)
}

/// A JSON answer for the multipart verbs.
fn json_response<B: serde::Serialize>(body: &B) -> Result<Response> {
    // why: these bodies are the deployment's own typed shapes, which always
    // serialize.
    let response = Response::ok(serde_json::to_string(body).expect("typed bodies serialize"))?;
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    Ok(response.with_headers(headers))
}

fn headers_get(req: &Request, name: &str) -> Result<Option<String>> {
    req.headers().get(name)
}

/// Read the request body as text; an unreadable body behaves like an
/// unparseable document for the caller.
async fn request_text(req: &mut Request) -> Result<String, ClientError> {
    req.text().await.map_err(|_| ClientError::MalformedNarinfo)
}

/// The two hashers and the decoder a NAR write threads its own body
/// through, so the bytes are measured on the way to the bucket instead of
/// read back out of it afterwards.
struct NarMeasure {
    compressed: Sha256Stream,
    nar: Sha256Stream,
    zstd: Option<ZstdStream>,
    refusal: Option<ClientError>,
}

impl NarMeasure {
    fn new(is_zst: bool, decode_bound: u64) -> Self {
        Self {
            compressed: Sha256Stream::new(),
            nar: Sha256Stream::new(),
            zstd: is_zst.then(|| ZstdStream::new(decode_bound)),
            refusal: None,
        }
    }

    /// What the measurement refused, once it has refused anything. The
    /// stream reports a decode failure as a stream error, which the
    /// runtime reports back as a storage failure; this is how the typed
    /// reason survives that translation.
    fn refusal(&self) -> Option<ClientError> {
        self.refusal
    }

    /// Take one chunk on its way past.
    fn feed(&mut self, chunk: &[u8]) -> std::result::Result<(), ClientError> {
        self.compressed.update(chunk);
        let Some(zstd) = self.zstd.as_mut() else {
            self.nar.update(chunk);
            return Ok(());
        };
        // why: the decode stays flat in the isolate's 128 MiB linear
        // memory. The body chunks itself however it likes, so feeds are
        // sliced here: a push never sees more than FEED_MAX of new input,
        // the pipe drains its consumed prefix, and the decoded output per
        // slice stays a few MiB no matter how large the object is.
        for piece in chunk.chunks(FEED_MAX) {
            match zstd.push(piece) {
                Ok(decoded) => self.nar.update(&decoded),
                Err(failure) => {
                    log::event(
                        "warn",
                        "write.nar_measure_failed",
                        &[
                            ("phase", "push".to_string()),
                            ("error", format!("{failure:?}")),
                            (
                                "compressedBytesRead",
                                self.compressed.byte_count().to_string(),
                            ),
                            ("decodedBytes", zstd.decompressed_bytes().to_string()),
                        ],
                    );
                    // why: a decode that fails cannot exonerate the
                    // narinfo that will name these bytes, so the refusal
                    // is the same one a wrong hash earns.
                    self.refusal = Some(ClientError::NarHashMismatch);
                    return Err(ClientError::NarHashMismatch);
                }
            }
        }
        Ok(())
    }

    /// Close the decoder and answer what the bytes measured.
    fn finish(&mut self) -> std::result::Result<NarFacts, ClientError> {
        if let Some(zstd) = self.zstd.as_mut() {
            match zstd.finish() {
                Ok(tail) => self.nar.update(&tail),
                Err(failure) => {
                    log::event(
                        "warn",
                        "write.nar_measure_failed",
                        &[
                            ("phase", "finish".to_string()),
                            ("error", format!("{failure:?}")),
                            (
                                "compressedBytesRead",
                                self.compressed.byte_count().to_string(),
                            ),
                            ("decodedBytes", zstd.decompressed_bytes().to_string()),
                        ],
                    );
                    self.refusal = Some(ClientError::NarHashMismatch);
                    return Err(ClientError::NarHashMismatch);
                }
            }
        }
        Ok(NarFacts {
            nar_hash: format!("sha256:{}", nix32_encode(&self.nar.digest_so_far())),
            nar_size_bytes: self.nar.byte_count(),
            file_hash_nix32: nix32_encode(&self.compressed.digest_so_far()),
            file_size_bytes: self.compressed.byte_count(),
        })
    }
}

/// A body on its way to the bucket, measured as it passes.
///
/// The bucket consumes a stream, so measuring means sitting between the
/// request and the bucket rather than reading the object back afterwards.
/// The chunks pass through untouched; only the hashers and the decoder
/// see them twice, and they see them while they are already in memory.
struct MeasuringStream {
    inner: Pin<Box<dyn Stream<Item = Result<Vec<u8>>>>>,
    measure: Rc<RefCell<NarMeasure>>,
}

impl MeasuringStream {
    fn new(
        body: impl Stream<Item = Result<Vec<u8>>> + 'static,
        measure: Rc<RefCell<NarMeasure>>,
    ) -> Self {
        Self {
            inner: Box::pin(body),
            measure,
        }
    }
}

impl Stream for MeasuringStream {
    type Item = Result<Vec<u8>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Every field is Unpin (a pinned box and a refcount), so the
        // stream moves freely and needs no projection.
        let this = self.get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Err(code) = this.measure.borrow_mut().feed(&chunk) {
                    return Poll::Ready(Some(Err(worker::Error::RustError(
                        code.code().to_string(),
                    ))));
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

/// Store a NAR in one request, measuring it on the way in.
///
/// Streamed straight through under `FixedLengthStream`: the runtime
/// receives the bytes, and a body whose real length disagrees with its
/// declared length fails the write instead of landing short. The bytes
/// are hashed and decoded as they pass, and the result lands beside the
/// object as its facts document, so the narinfo that names this NAR can
/// verify it without reading it again.
pub async fn put_nar(env: &Env, mut req: Request, key: &NarKey) -> Result<Response> {
    let length = match require_content_length(
        headers_get(&req, "content-length")?.as_deref(),
        UPLOAD_SINGLE_MAX_BYTES,
    ) {
        Ok(length) => length,
        Err(code) => return rejected(code),
    };
    let declared_nar_bytes = match require_content_length(
        headers_get(&req, NAR_DECLARED_BYTES_HEADER)?.as_deref(),
        NAR_DECOMPRESSED_BYTES_MAX,
    ) {
        Ok(declared) => declared,
        Err(code) => return rejected(code),
    };
    let Ok(body) = req.stream() else {
        return rejected(ClientError::LengthRequired);
    };
    let measure = Rc::new(RefCell::new(NarMeasure::new(
        key.suffix() == ".zst",
        nar_decode_bound(length, declared_nar_bytes),
    )));
    // A repeated write is harmless: objects are content-addressed, so a
    // retry stores identical bytes under an identical key.
    let bucket = env.bucket("CACHE_BUCKET")?;
    let stored = bucket
        .put(
            key.as_str(),
            FixedLengthStream::wrap(MeasuringStream::new(body, Rc::clone(&measure)), length),
        )
        .execute()
        .await;
    if let Err(failure) = stored {
        // why: the measurement's own refusal outranks the storage error it
        // caused. A decoder that refused mid-body aborts the stream, which
        // the bucket reports as a failed put; answering 503 there would
        // tell the client to retry bytes the server will refuse again.
        let refusal = measure.borrow().refusal();
        if let Some(code) = refusal {
            log::event(
                "warn",
                "write.nar_rejected",
                &[("code", code.code().to_string())],
            );
            return rejected(code);
        }
        log::event(
            "error",
            "write.nar_store_failed",
            &[("error", failure.to_string())],
        );
        return rejected(ClientError::StorageUnavailable);
    }
    // The borrow closes before the refusal path awaits: a RefCell guard
    // must never span an await, and the failure branch deletes.
    let measured = measure.borrow_mut().finish();
    let facts = match measured {
        Ok(facts) => facts,
        Err(code) => return discard_nar(&bucket, key, code).await,
    };
    // Content addressing, checked where it is cheapest: a NAR key names
    // the hash of the bytes it holds, so bytes that hash to anything else
    // do not own this key. Storing them would leave an object no narinfo
    // can ever verify, waiting for the collector.
    if facts.file_hash_nix32 != key.file_hash() {
        log::event(
            "warn",
            "write.nar_rejected",
            &[
                ("code", ClientError::FileHashMismatch.code().to_string()),
                ("fileHashNamed", key.file_hash().to_string()),
                ("fileHashMeasured", facts.file_hash_nix32.clone()),
            ],
        );
        return discard_nar(&bucket, key, ClientError::FileHashMismatch).await;
    }
    if let Err(failure) = bucket
        .put(facts_key(key), facts.serialize())
        .execute()
        .await
    {
        // The object is stored but unverifiable without its facts, so the
        // write did not succeed: the client retries, and the collector
        // reaps the orphan if it never does.
        log::event(
            "error",
            "write.nar_facts_store_failed",
            &[("error", failure.to_string())],
        );
        return rejected(ClientError::StorageUnavailable);
    }
    log::event(
        "info",
        "write.nar_stored",
        &[
            ("sizeBytes", length.to_string()),
            ("narSizeBytes", facts.nar_size_bytes.to_string()),
        ],
    );
    stored_response()
}

/// Delete a NAR the measurement refused, then answer the refusal. The
/// delete is best effort: an object with no facts beside it can never be
/// signed, and the collector reaps it.
async fn discard_nar(bucket: &worker::Bucket, key: &NarKey, code: ClientError) -> Result<Response> {
    if let Err(failure) = bucket.delete(key.as_str()).await {
        log::event(
            "warn",
            "write.nar_discard_failed",
            &[("error", failure.to_string())],
        );
    }
    rejected(code)
}

/// Begin a multipart upload: the client declares the total, the plan fixes
/// every part's size, and the bookkeeping record lands before the id is
/// handed out — an upload whose record failed to store is aborted
/// immediately rather than left usable but unverifiable.
pub async fn create_multipart(
    env: &Env,
    key: &NarKey,
    now: UnixMillis,
    req: &mut Request,
) -> Result<Response> {
    let declared = match require_content_length(
        headers_get(req, UPLOAD_TOTAL_BYTES_HEADER)?.as_deref(),
        u64::MAX,
    ) {
        Ok(total) => total,
        Err(code) => return rejected(code),
    };
    let declared_nar_bytes = match require_content_length(
        headers_get(req, NAR_DECLARED_BYTES_HEADER)?.as_deref(),
        NAR_DECOMPRESSED_BYTES_MAX,
    ) {
        Ok(declared) => declared,
        Err(code) => return rejected(code),
    };
    let shape = match cachet_core::multipart::plan_shape(declared) {
        Ok(shape) => shape,
        Err(code) => return rejected(code),
    };
    let bucket = env.bucket("CACHE_BUCKET")?;
    let upload = bucket
        .create_multipart_upload(key.as_str())
        .execute()
        .await?;
    let upload_id = upload.upload_id().await;
    let record = UploadRecord {
        key: key.as_str().to_string(),
        total_bytes: declared,
        expected_parts: shape.count,
        nar_bytes: declared_nar_bytes,
        created_at_ms: now.as_u64(),
    };
    if let Err(failure) = bucket
        .put(upload_record_key(&upload_id), record.serialize())
        .execute()
        .await
    {
        let _ = upload.abort().await;
        log::event(
            "error",
            "write.upload_record_failed",
            &[("error", failure.to_string())],
        );
        return rejected(ClientError::StorageUnavailable);
    }
    log::event(
        "info",
        "write.multipart_created",
        &[("expectedParts", shape.count.to_string())],
    );
    json_response(&cachet_api::UploadCreated {
        upload_id,
        expected_parts: shape.count,
    })
}

/// Load the bookkeeping record for an upload id, translatable failures
/// only: absent objects and unparseable or key-foreign records all read as
/// `upload_unknown`, because a client can only cause them by presenting an
/// id that does not name what it claims.
async fn load_record(
    env: &Env,
    key: &NarKey,
    upload_id: &str,
) -> Result<UploadRecord, ClientError> {
    let bucket = env
        .bucket("CACHE_BUCKET")
        .map_err(|_| ClientError::StorageUnavailable)?;
    let Some(object) = bucket
        .get(upload_record_key(upload_id))
        .execute()
        .await
        // why: the bucket's own error text is the only witness when a
        // get fails intermittently; folding it silently into 503 leaves
        // the lane blind.
        .map_err(|failure| {
            log::event(
                "error",
                "write.record_load_failed",
                &[("error", failure.to_string())],
            );
            ClientError::StorageUnavailable
        })?
    else {
        return Err(ClientError::UploadUnknown);
    };
    let Some(body) = object.body() else {
        log::event(
            "error",
            "write.record_load_failed",
            &[("error", "object had no body".to_string())],
        );
        return Err(ClientError::StorageUnavailable);
    };
    let text = body.text().await.map_err(|failure| {
        log::event(
            "error",
            "write.record_load_failed",
            &[("error", failure.to_string())],
        );
        ClientError::StorageUnavailable
    })?;
    let record = UploadRecord::parse(&text).map_err(|_| ClientError::UploadUnknown)?;
    // The upload id is a bearer token for one key: a record naming a
    // different key is refused, so a client cannot present one upload's id
    // while writing to another object.
    if record.key != key.as_str() {
        return Err(ClientError::UploadUnknown);
    }
    Ok(record)
}

/// Store one part.
///
/// The size check runs here, per request, rather than at completion: a
/// wrong-sized part costs one request instead of the whole upload.
pub async fn upload_part(
    env: &Env,
    mut req: Request,
    key: &NarKey,
    upload_id: &str,
    part_number: u64,
) -> Result<Response> {
    let length = match require_content_length(
        headers_get(&req, "content-length")?.as_deref(),
        UPLOAD_SINGLE_MAX_BYTES,
    ) {
        Ok(length) => length,
        Err(code) => return rejected(code),
    };
    let record = match load_record(env, key, upload_id).await {
        Ok(record) => record,
        Err(code) => return rejected(code),
    };
    if let Err(code) = check_part(&record, part_number, length) {
        log::event(
            "warn",
            "write.part_rejected",
            &[
                ("partNumber", part_number.to_string()),
                ("code", code.code().to_string()),
            ],
        );
        return rejected(code);
    }
    // why: the body stream is adopted only once every byte-free check has
    // passed, so a refused part never leaves the client's upload body
    // pinned open on the connection.
    let Ok(body) = req.stream() else {
        return rejected(ClientError::LengthRequired);
    };
    let bucket = env.bucket("CACHE_BUCKET")?;
    let upload = bucket.resume_multipart_upload(key.as_str(), upload_id)?;
    // Safe: check_part established the bound 1..=expectedParts ≤ 1000.
    let part_u16 = u16::try_from(part_number).expect("part numbers fit u16 by the plan cap");
    match upload
        .upload_part(part_u16, FixedLengthStream::wrap(body, length))
        .await
    {
        Ok(part) => json_response(&cachet_api::UploadedPartBody {
            part_number: part.part_number(),
            etag: part.etag(),
        }),
        Err(failure) => {
            log::event(
                "error",
                "write.part_store_failed",
                &[("error", failure.to_string())],
            );
            rejected(ClientError::StorageUnavailable)
        }
    }
}

/// Assemble the parts. Completion is idempotent on replay: if the record
/// is gone but the object exists, the upload succeeded and the client did
/// not hear the answer; content addressing makes that safe to assume.
pub async fn complete_multipart(
    env: &Env,
    mut req: Request,
    key: &NarKey,
    upload_id: &str,
) -> Result<Response> {
    let record = match load_record(env, key, upload_id).await {
        Ok(record) => record,
        Err(code) => {
            if code == ClientError::UploadUnknown {
                let bucket = env.bucket("CACHE_BUCKET")?;
                if bucket.head(key.as_str()).await?.is_some() {
                    return stored_response();
                }
            }
            return rejected(code);
        }
    };
    let _completion_length = match require_content_length(
        headers_get(&req, "content-length")?.as_deref(),
        COMPLETE_BODY_BYTES_MAX,
    ) {
        Ok(length) => length,
        Err(code) => return rejected(code),
    };
    let text = match request_text(&mut req).await {
        Ok(text) => text,
        Err(code) => return rejected(code),
    };
    let parts = match parse_completion_body(&text, &record) {
        Ok(parts) => parts,
        Err(code) => return rejected(code),
    };
    let bucket = env.bucket("CACHE_BUCKET")?;
    let upload = bucket.resume_multipart_upload(key.as_str(), upload_id)?;
    let uploaded_parts: Vec<worker::UploadedPart> = parts
        .iter()
        .map(|part| {
            worker::UploadedPart::new(
                u16::try_from(part.number).expect("bounded by the plan cap"),
                part.etag.clone(),
            )
        })
        .collect();
    if let Err(failure) = upload.complete(uploaded_parts).await {
        log::event(
            "error",
            "write.complete_failed",
            &[("error", failure.to_string())],
        );
        return rejected(ClientError::StorageUnavailable);
    }
    // The record is bookkeeping: the object is the truth now, and a stale
    // record only costs the collector one stale-upload check later.
    if let Err(failure) = bucket.delete(upload_record_key(upload_id)).await {
        log::event(
            "warn",
            "write.record_delete_failed",
            &[("error", failure.to_string())],
        );
    }
    // The one place a NAR is still read back to be measured. Parts arrive
    // out of order and in parallel, so nothing sat between them and the
    // bucket the way a single-shot write does; the assembled object is
    // the first time these bytes exist in order. Paying it here rather
    // than on the narinfo request keeps it to one read of the objects
    // large enough to need multipart at all, and the narinfo that follows
    // reads the facts instead of the bytes.
    match measure_stored_nar(&bucket, key, record.nar_bytes).await {
        Ok(facts) => {
            if facts.file_hash_nix32 != key.file_hash() {
                log::event(
                    "warn",
                    "write.nar_rejected",
                    &[
                        ("code", ClientError::FileHashMismatch.code().to_string()),
                        ("fileHashNamed", key.file_hash().to_string()),
                        ("fileHashMeasured", facts.file_hash_nix32.clone()),
                    ],
                );
                return discard_nar(&bucket, key, ClientError::FileHashMismatch).await;
            }
            if let Err(failure) = bucket
                .put(facts_key(key), facts.serialize())
                .execute()
                .await
            {
                log::event(
                    "error",
                    "write.nar_facts_store_failed",
                    &[("error", failure.to_string())],
                );
                return rejected(ClientError::StorageUnavailable);
            }
            log::event(
                "info",
                "write.multipart_completed",
                &[("narSizeBytes", facts.nar_size_bytes.to_string())],
            );
            stored_response()
        }
        Err(code) => discard_nar(&bucket, key, code).await,
    }
}

/// Read an assembled object back and measure it, for the multipart path
/// that could not measure it on the way in.
async fn measure_stored_nar(
    bucket: &worker::Bucket,
    key: &NarKey,
    declared_nar_bytes: u64,
) -> std::result::Result<NarFacts, ClientError> {
    let Some(object) = bucket
        .get(key.as_str())
        .execute()
        .await
        .map_err(|_| ClientError::StorageUnavailable)?
    else {
        return Err(ClientError::StorageUnavailable);
    };
    let size = object.size();
    let Some(body) = object.body() else {
        return Err(ClientError::StorageUnavailable);
    };
    let mut measure = NarMeasure::new(
        key.suffix() == ".zst",
        nar_decode_bound(size, declared_nar_bytes),
    );
    let mut stream = body.stream().map_err(|_| ClientError::StorageUnavailable)?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ClientError::StorageUnavailable)?;
        measure.feed(&chunk)?;
    }
    measure.finish()
}

/// Discard an upload.
pub async fn abort_multipart(env: &Env, key: &NarKey, upload_id: &str) -> Result<Response> {
    if let Err(code) = load_record(env, key, upload_id).await {
        return rejected(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;
    bucket
        .resume_multipart_upload(key.as_str(), upload_id)?
        .abort()
        .await
        .inspect_err(|failure| {
            log::event(
                "error",
                "write.abort_failed",
                &[("error", failure.to_string())],
            );
        })?;
    let _ = bucket.delete(upload_record_key(upload_id)).await;
    stored_response()
}

/// Store a narinfo, but only after its NAR verifies byte-for-byte.
///
/// The pipeline is the defense, and it runs in two requests rather than
/// one. The NAR's own write hashed it compressed and decoded-hashed it
/// decompressed as the bytes passed, and recorded the result beside the
/// object; this request reads those facts, checks the document's claims
/// against them, and only then lets the signing key see the fingerprint.
/// A client that lies about any byte of the NAR gets a typed refusal,
/// never a signed narinfo. The facts document is written only for a NAR
/// that was measured in full, so its presence is what makes the order
/// verify-then-sign (CLAUDE.md §1).
pub async fn put_narinfo(
    env: &Env,
    mut req: Request,
    request_hash: &StorePathHash,
) -> Result<Response> {
    let _length = match require_content_length(
        headers_get(&req, "content-length")?.as_deref(),
        NARINFO_BYTES_MAX,
    ) {
        Ok(length) => length,
        Err(code) => return rejected(code),
    };
    let text = match request_text(&mut req).await {
        Ok(text) => text,
        Err(code) => return rejected(code),
    };
    let document = match Narinfo::parse(&text) {
        Ok(document) => document,
        Err(code) => return rejected(code),
    };
    if let Err(code) = check_narinfo_claims(&document, request_hash) {
        return rejected(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;

    // The write-time half of NEVER-DANGLE: the uploader's NAR-first
    // ordering is verified, not trusted. The facts document answers it,
    // because it exists only for a NAR whose bytes were stored and
    // measured in full, so its presence proves the NAR's.
    let facts = match read_nar_facts(&bucket, &document.url).await {
        Ok(Some(facts)) => facts,
        Ok(None) => {
            log::event(
                "warn",
                "write.narinfo_rejected",
                &[("code", ClientError::NarinfoNarMissing.code().to_string())],
            );
            return rejected(ClientError::NarinfoNarMissing);
        }
        Err(code) => return rejected(code),
    };

    let verified = match VerifiedNar::verify(
        &document,
        facts.nar_size_bytes,
        &facts.nar_hash,
        &facts.file_hash_nix32,
        facts.file_size_bytes,
    ) {
        Ok(verified) => verified,
        Err(code) => {
            // why: problem bodies never carry occurrence specifics (the
            // law in cachet-core/src/problem.rs), so the operator's answer
            // for a disagreement lives here, in the operator-facing event:
            // every compared pair, declared against measured.
            let mut fields = vec![("code", code.code().to_string())];
            if matches!(
                code,
                ClientError::NarHashMismatch | ClientError::FileHashMismatch
            ) {
                fields.extend([
                    ("narHashDeclared", document.nar_hash.as_str().to_string()),
                    ("narHashMeasured", facts.nar_hash.clone()),
                    ("narSizeDeclared", document.nar_size_bytes.to_string()),
                    ("narSizeMeasured", facts.nar_size_bytes.to_string()),
                    (
                        "fileHashDeclared",
                        document.file_hash.clone().unwrap_or_default(),
                    ),
                    (
                        "fileHashMeasured",
                        format!("sha256:{}", facts.file_hash_nix32),
                    ),
                    (
                        "fileSizeDeclared",
                        document
                            .file_size_bytes
                            .map(|size| size.to_string())
                            .unwrap_or_default(),
                    ),
                    ("fileSizeMeasured", facts.file_size_bytes.to_string()),
                ]);
            }
            log::event("warn", "write.narinfo_rejected", &fields);
            return rejected(code);
        }
    };

    sign_and_store(env, &bucket, &verified, request_hash).await
}

/// Read what a stored NAR measured. `Ok(None)` means no such NAR has been
/// stored and measured, which is the only answer a narinfo naming it can
/// act on.
async fn read_nar_facts(
    bucket: &worker::Bucket,
    key: &NarKey,
) -> std::result::Result<Option<NarFacts>, ClientError> {
    let Some(object) = bucket
        .get(facts_key(key))
        .execute()
        .await
        .map_err(|failure| {
            log::event(
                "error",
                "write.nar_facts_get_failed",
                &[("error", failure.to_string())],
            );
            ClientError::StorageUnavailable
        })?
    else {
        return Ok(None);
    };
    if object.size() > NAR_FACTS_BYTES_MAX {
        log::event(
            "error",
            "write.nar_facts_oversized",
            &[("sizeBytes", object.size().to_string())],
        );
        return Err(ClientError::StorageUnavailable);
    }
    let Some(body) = object.body() else {
        return Err(ClientError::StorageUnavailable);
    };
    let text = body
        .text()
        .await
        .map_err(|_| ClientError::StorageUnavailable)?;
    // The worker wrote this document; an unparseable one is a storage
    // fault, never something the client did.
    NarFacts::parse(&text).map(Some).map_err(|failure| {
        log::event(
            "error",
            "write.nar_facts_corrupt",
            &[("error", failure.to_string())],
        );
        ClientError::StorageUnavailable
    })
}

/// Sign the verified document and store it under the request's key. The
/// signing key's name lives in its own document (bootstrap generated it as
/// `{host}-1`), so Sig lines name the deployment without a config value
/// duplicating it (ADR 0013).
async fn sign_and_store(
    env: &Env,
    bucket: &worker::Bucket,
    verified: &VerifiedNar,
    request_hash: &StorePathHash,
) -> Result<Response> {
    let document = verified.document();
    let key_text = match env.secret("CACHET_SIGNING_KEY") {
        Ok(secret) => secret.to_string(),
        Err(failure) => {
            log::event(
                "error",
                "write.signing_key_missing",
                &[("error", failure.to_string())],
            );
            return Err(failure);
        }
    };
    let signing_key = match NixSecretKey::parse(&key_text) {
        Ok(key) => key,
        Err(failure) => {
            log::event(
                "error",
                "write.signing_key_corrupt",
                &[("error", format!("{failure:?}"))],
            );
            return rejected(ClientError::StorageUnavailable);
        }
    };
    let signature = signing_key.sign_fingerprint(&document.fingerprint());
    let stored = document.with_signature(signature).serialize();
    match bucket
        .put(format!("{request_hash}.narinfo"), stored.clone())
        .execute()
        .await
    {
        Ok(_) => {
            log::event(
                "info",
                "write.narinfo_stored",
                &[("sizeBytes", stored.len().to_string())],
            );
            stored_response()
        }
        Err(failure) => {
            log::event(
                "error",
                "write.narinfo_store_failed",
                &[("error", failure.to_string())],
            );
            rejected(ClientError::StorageUnavailable)
        }
    }
}

/// One decoder feed's upper size: with the pipe draining its consumed
/// prefix, every in-flight allocation stays a few MiB at any object size.
const FEED_MAX: usize = 4 * 1024 * 1024;
