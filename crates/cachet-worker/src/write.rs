//! Accepting objects: the single PUT, the multipart sequence, and the
//! verify-then-sign pipeline for narinfos. The guards, plan, claim checks,
//! and completion grammar are cachet-core's; the hashing, decoding, and
//! signing are cachet-crypto's; this module performs the bucket and
//! request I/O in exactly that order. Two invariants are enforced here
//! rather than trusted: a narinfo is stored only after its NAR verifies
//! byte-for-byte (NEVER-DANGLE), and the signing step accepts only the
//! verifier's output (verify-then-sign).

use cachet_core::constants::{
    COMPLETE_BODY_BYTES_MAX, NARINFO_BYTES_MAX, UPLOAD_SINGLE_MAX_BYTES, UPLOADS_KEY_PREFIX,
};
use cachet_core::error::ClientError;
use cachet_core::keys::NarKey;
use cachet_core::multipart::{check_part, parse_completion_body};
use cachet_core::narinfo::Narinfo;
use cachet_core::types::{StorePathHash, UnixMillis};
use cachet_core::upload_record::UploadRecord;
use cachet_core::write::{VerifiedNar, check_narinfo_claims, require_content_length};
use cachet_crypto::base32::encode as nix32_encode;
use cachet_crypto::ed25519::NixSecretKey;
use cachet_crypto::sha256::Sha256Stream;
use cachet_crypto::zstd_stream::ZstdStream;
use futures_util::StreamExt as _;
use worker::{Env, FixedLengthStream, ObjectBody, Request, Response, Result};

use crate::{error, log};

/// The header a multipart client declares the total size in.
const UPLOAD_TOTAL_BYTES_HEADER: &str = "x-cachet-upload-bytes";

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

/// Store a NAR in one request.
///
/// Streamed straight through under `FixedLengthStream`: the runtime
/// receives the bytes, and a body whose real length disagrees with its
/// declared length fails the write instead of landing short.
pub async fn put_nar(env: &Env, mut req: Request, key: &NarKey) -> Result<Response> {
    let length = match require_content_length(
        headers_get(&req, "content-length")?.as_deref(),
        UPLOAD_SINGLE_MAX_BYTES,
    ) {
        Ok(length) => length,
        Err(code) => return rejected(code),
    };
    let Ok(body) = req.stream() else {
        return rejected(ClientError::LengthRequired);
    };
    // A repeated write is harmless: objects are content-addressed, so a
    // retry stores identical bytes under an identical key.
    let bucket = env.bucket("CACHE_BUCKET")?;
    match bucket
        .put(key.as_str(), FixedLengthStream::wrap(body, length))
        .execute()
        .await
    {
        Ok(_) => {
            log::event(
                "info",
                "write.nar_stored",
                &[("sizeBytes", length.to_string())],
            );
            stored_response()
        }
        Err(failure) => {
            log::event(
                "error",
                "write.nar_store_failed",
                &[("error", failure.to_string())],
            );
            rejected(ClientError::StorageUnavailable)
        }
    }
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
    match upload.complete(uploaded_parts).await {
        Ok(_) => {
            // The record is bookkeeping: the object is the truth now, and
            // a stale record only costs the collector one stale-upload
            // check later.
            if let Err(failure) = bucket.delete(upload_record_key(upload_id)).await {
                log::event(
                    "warn",
                    "write.record_delete_failed",
                    &[("error", failure.to_string())],
                );
            }
            log::event("info", "write.multipart_completed", &[]);
            stored_response()
        }
        Err(failure) => {
            log::event(
                "error",
                "write.complete_failed",
                &[("error", failure.to_string())],
            );
            rejected(ClientError::StorageUnavailable)
        }
    }
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
/// The pipeline is the defense: the stored object is re-hashed compressed
/// and decoded-hashed decompressed, its claims are checked against the
/// document, and only then does the signing key see the fingerprint. A
/// client that lies about any byte of the NAR gets a typed refusal, never
/// a signed narinfo.
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
    // ordering is verified, not trusted.
    if bucket.head(document.url.as_str()).await?.is_none() {
        log::event(
            "warn",
            "write.narinfo_rejected",
            &[("code", ClientError::NarinfoNarMissing.code().to_string())],
        );
        return rejected(ClientError::NarinfoNarMissing);
    }

    let is_zst = document.url.suffix() == ".zst";
    let Some(object) = bucket
        .get(document.url.as_str())
        .execute()
        .await
        .inspect_err(|failure| {
            log::event(
                "error",
                "write.verify_get_failed",
                &[("error", failure.to_string())],
            );
        })?
    else {
        // A head race the bucket resolved against us: the NAR vanished
        // between the check and the read.
        return rejected(ClientError::NarinfoNarMissing);
    };
    let Some(body) = object.body() else {
        return rejected(ClientError::StorageUnavailable);
    };
    let measured = match measure_nar(body, is_zst, document.nar_size_bytes).await {
        Ok(measured) => measured,
        Err(code) => return rejected(code),
    };
    let (decompressed_size, decompressed_hash_text, compressed_hash_nix32, compressed_size) =
        measured;
    let verified = match VerifiedNar::verify(
        &document,
        decompressed_size,
        &decompressed_hash_text,
        &compressed_hash_nix32,
        compressed_size,
    ) {
        Ok(verified) => verified,
        Err(code) => {
            log::event(
                "warn",
                "write.narinfo_rejected",
                &[("code", code.code().to_string())],
            );
            return rejected(code);
        }
    };

    sign_and_store(env, &bucket, &verified, request_hash).await
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

/// Stream the stored NAR through two hashers — the compressed bytes
/// themselves and, for zstd, their decoding — returning
/// (decompressed size, decompressed `sha256:<nix32>` text, bare compressed
/// nix32 hash, compressed size). A corrupt frame or a stream over its
/// declared NarSize maps onto the nar-claim mismatch the document has
/// coming.
async fn measure_nar(
    body: ObjectBody<'_>,
    is_zst: bool,
    nar_size_limit: u64,
) -> Result<(u64, String, String, u64), ClientError> {
    let mut compressed = Sha256Stream::new();
    let mut nar = Sha256Stream::new();
    let mut zstd = ZstdStream::new(nar_size_limit);
    let mut stream = body.stream().map_err(|_| ClientError::StorageUnavailable)?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ClientError::StorageUnavailable)?;
        compressed.update(&chunk);
        if is_zst {
            let decoded = zstd
                .push(&chunk)
                .map_err(|_| ClientError::NarHashMismatch)?;
            nar.update(&decoded);
        } else {
            nar.update(&chunk);
        }
    }
    if is_zst {
        let tail = zstd.finish().map_err(|_| ClientError::NarHashMismatch)?;
        nar.update(&tail);
    }
    Ok((
        nar.byte_count(),
        format!("sha256:{}", nix32_encode(&nar.digest_so_far())),
        nix32_encode(&compressed.digest_so_far()),
        compressed.byte_count(),
    ))
}
