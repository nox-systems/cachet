//! The route descriptors. These functions are complete as written: their
//! entire behavior is to carry the `#[utoipa::path]` contracts the
//! document is derived from, so there is nothing for a caller to invoke.
//! The request and response bodies the worker actually serializes are
//! the shared types in the crate root; anything a description names that
//! the worker would not answer fails review in the workerd lane first.

use crate::{
    GcRunList, ProblemBody, ProjectList, PublicConfig, RenewalBody, StatsBody, UploadCreated,
    UploadedPartBody,
};

/// `GET /nix-cache-info`: the nix handshake. Immutable per deployment
/// generation; always 200 because it describes configuration, not data.
#[utoipa::path(
    get,
    path = "/nix-cache-info",
    responses(
        (status = 200, description = "StoreDir, WantMassQuery, and Priority, content-type text/x-nix-cache-info", content_type = "text/x-nix-cache-info"),
    )
)]
pub fn cache_info_get() {}

/// `HEAD /nix-cache-info`: header parity with the GET.
#[utoipa::path(
    head,
    path = "/nix-cache-info",
    responses(
        (status = 200, description = "The GET's headers without the body"),
    )
)]
pub fn cache_info_head() {}

/// `GET /{hash}.narinfo`: one narinfo document, edge-cached for 30 days
/// and byte-immutable. Reads authenticate like every cache read; the
/// narinfo's signatures are the content-integrity check, not the gate.
#[utoipa::path(
    get,
    path = "/{hash}.narinfo",
    params(
        ("hash" = String, Path, description = "The 32-character nix base32 store-path hash"),
    ),
    responses(
        (status = 200, description = "The signed narinfo", content_type = "text/x-nix-narinfo"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn narinfo_get() {}

/// `HEAD /{hash}.narinfo`: metadata only; bypasses the edge cache so a
/// client can ask what is really stored without evicting anything.
#[utoipa::path(
    head,
    path = "/{hash}.narinfo",
    params(
        ("hash" = String, Path, description = "The 32-character nix base32 store-path hash"),
    ),
    responses(
        (status = 200, description = "The GET's headers without the body"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn narinfo_head() {}

/// `PUT /{hash}.narinfo`: the verify-then-sign pipeline. The stored NAR
/// is re-hashed compressed and decompressed before anything is signed.
#[utoipa::path(
    put,
    path = "/{hash}.narinfo",
    params(
        ("hash" = String, Path, description = "The 32-character nix base32 store-path hash"),
    ),
    request_body(
        description = "The narinfo, as nix wrote it; unsigned or client-signed",
        content_type = "text/x-nix-narinfo",
        content = String,
    ),
    responses(
        (status = 204, description = "Stored, signed with the deployment key"),
        (status = 400, description = "problem+json; code=malformed_key, malformed_narinfo, store_path_mismatch, unsupported_compression, nar_hash_mismatch, or file_hash_mismatch", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "problem+json; code=narinfo_nar_missing", body = ProblemBody, content_type = "application/problem+json"),
        (status = 411, description = "problem+json; code=length_required", body = ProblemBody, content_type = "application/problem+json"),
        (status = 413, description = "problem+json; code=body_too_large", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn narinfo_put() {}

/// `GET /nar/{key}`: one NAR, edge-cached for 30 days, byte-immutable.
/// Requires a read credential: a GitHub token as Bearer (or Basic
/// password, which is what nix's netrc support sends), an OIDC token
/// (which is what CI runners carry), or the browser session cookie.
#[utoipa::path(
    get,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key: the 52-character sha256 nix base32 with an optional compression suffix"),
    ),
    responses(
        (status = 200, description = "The NAR bytes", content_type = "application/x-nix-nar"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn nar_get() {}

/// `HEAD /nar/{key}`: metadata only; bypasses the edge cache.
#[utoipa::path(
    head,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key"),
    ),
    responses(
        (status = 200, description = "The GET's headers without the body"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn nar_head() {}

/// `GET /api/public/config`: discovery for the CLI and the browser flow.
/// Unauthenticated and uncacheable.
#[utoipa::path(
    get,
    path = "/api/public/config",
    responses(
        (status = 200, description = "The deployment's public configuration; cache-control no-store", body = PublicConfig),
        (status = 503, description = "problem+json; code=auth_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn public_config_get() {}

/// `GET /api/openapi.json`: this document, served as the committed yaml
/// so the served bytes are the drift-checked bytes.
#[utoipa::path(
    get,
    path = "/api/openapi.json",
    responses(
        (status = 200, description = "The generated OpenAPI document, byte-identical to the committed docs/openapi.yaml", content_type = "application/yaml"),
    )
)]
pub fn openapi_get() {}

/// `GET /roots`: the projects holding leases. Requires a read credential.
#[utoipa::path(
    get,
    path = "/roots",
    responses(
        (status = 200, description = "The leased projects; cache-control no-store", body = ProjectList),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn projects_list() {}

/// `GET /roots/{project}`: one lease document as it was renewed. Requires
/// a read credential.
#[utoipa::path(
    get,
    path = "/roots/{project}",
    params(
        ("project" = String, Path, description = "The hyphenated owner-repo project name"),
    ),
    responses(
        (status = 200, description = "The lease document the renewal stored; cache-control no-store", content_type = "application/json"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn lease_get() {}

/// `POST /roots/{project}`: renew a lease. Claims (ref, repository, run,
/// commit) come from the verified OIDC token, never from the body, and
/// the project must be the token's own repository.
#[utoipa::path(
    post,
    path = "/roots/{project}",
    params(
        ("project" = String, Path, description = "The hyphenated owner-repo project name"),
    ),
    request_body(
        description = "The store paths to keep and the installables that produced them",
        content_type = "application/json",
        content = RenewalBody,
    ),
    responses(
        (status = 204, description = "The lease renewed"),
        (status = 400, description = "problem+json; code=malformed_key or malformed_roots", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org, forbidden_ref, or forbidden_project", body = ProblemBody, content_type = "application/problem+json"),
        (status = 411, description = "problem+json; code=length_required", body = ProblemBody, content_type = "application/problem+json"),
        (status = 413, description = "problem+json; code=body_too_large", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn lease_renew() {}

/// `PUT /nar/{key}`: a whole NAR in one request, up to the single-PUT
/// cap. Larger uploads use the multipart quartet below.
#[utoipa::path(
    put,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key: sha256 nix base32 with an optional compression suffix"),
    ),
    request_body(
        description = "The NAR bytes; the key names their sha256, so the content is self-addressing",
        content_type = "application/x-nix-nar",
        content = String,
    ),
    responses(
        (status = 204, description = "Stored"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 411, description = "problem+json; code=length_required", body = ProblemBody, content_type = "application/problem+json"),
        (status = 413, description = "problem+json; code=body_too_large", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn nar_put() {}

/// `POST /nar/{key}?uploads`: begin a multipart upload. The
/// `x-cachet-upload-bytes` header declares the total size and binds the
/// upload's part plan.
#[utoipa::path(
    post,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key"),
        ("uploads" = bool, Query, description = "Creates a multipart upload"),
        ("x-cachet-upload-bytes" = String, Header, description = "The declared total size in bytes"),
    ),
    responses(
        (status = 200, description = "The upload id", body = UploadCreated),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 411, description = "problem+json; code=length_required", body = ProblemBody, content_type = "application/problem+json"),
        (status = 413, description = "problem+json; code=body_too_large", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn upload_create() {}

/// `PUT /nar/{key}?uploadId&partNumber`: one part, exactly the plan's
/// uniform size except the final part.
#[utoipa::path(
    put,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key"),
        ("uploadId" = String, Query, description = "The upload the part belongs to"),
        ("partNumber" = u32, Query, description = "The one-based part number"),
    ),
    request_body(
        description = "The part bytes",
        content_type = "application/octet-stream",
        content = String,
    ),
    responses(
        (status = 200, description = "The part's etag, for the completion body", body = UploadedPartBody),
        (status = 400, description = "problem+json; code=malformed_key, part_number_invalid, or part_size_mismatch", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=upload_unknown", body = ProblemBody, content_type = "application/problem+json"),
        (status = 411, description = "problem+json; code=length_required", body = ProblemBody, content_type = "application/problem+json"),
        (status = 413, description = "problem+json; code=body_too_large", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn upload_part_put() {}

/// `POST /nar/{key}?uploadId`: complete the upload with exactly the
/// expected part set. Replay-idempotent: repeating the same completion
/// after a lost response answers 204 again.
#[utoipa::path(
    post,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key"),
        ("uploadId" = String, Query, description = "The upload to complete"),
    ),
    request_body(
        description = "The parts as [{\"partNumber\": N, \"etag\": \"...\"}], ascending and gapless",
        content_type = "application/json",
        content = String,
    ),
    responses(
        (status = 204, description = "Assembled and stored"),
        (status = 400, description = "problem+json; code=malformed_key or complete_parts_mismatch", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=upload_unknown", body = ProblemBody, content_type = "application/problem+json"),
        (status = 411, description = "problem+json; code=length_required", body = ProblemBody, content_type = "application/problem+json"),
        (status = 413, description = "problem+json; code=body_too_large", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn upload_complete() {}

/// `DELETE /nar/{key}?uploadId`: abandon an upload and let the storage
/// reclaim its parts.
#[utoipa::path(
    delete,
    path = "/nar/{key}",
    params(
        ("key" = String, Path, description = "The NAR object key"),
        ("uploadId" = String, Query, description = "The upload to abort"),
    ),
    responses(
        (status = 204, description = "Aborted"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=upload_unknown", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn upload_abort() {}

/// `GET /api/self/gc-runs`: one page of run ids, oldest first. Admins
/// only: the login from the read credential must enter CACHET_ADMINS.
#[utoipa::path(
    get,
    path = "/api/self/gc-runs",
    params(
        ("cursor" = Option<String>, Query, description = "The nextCursor from the previous page"),
    ),
    responses(
        (status = 200, description = "The run ids, oldest first; cache-control no-store", body = GcRunList),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_admin", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn gc_runs_list() {}

/// `GET /api/self/gc-runs/{runId}`: one run's full report, as stored.
#[utoipa::path(
    get,
    path = "/api/self/gc-runs/{runId}",
    params(
        ("runId" = String, Path, description = "The run id: milliseconds, a dash, sixteen lowercase hex characters"),
    ),
    responses(
        (status = 200, description = "The run's report document; cache-control no-store", content_type = "application/json"),
        (status = 400, description = "problem+json; code=malformed_key", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_admin", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn gc_run_get() {}

/// `GET /api/self/stats`: the cache's current shape, derived from the
/// newest completed report.
#[utoipa::path(
    get,
    path = "/api/self/stats",
    responses(
        (status = 200, description = "The totals from the newest report; cache-control no-store", body = StatsBody),
        (status = 401, description = "problem+json; code=unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_admin", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "problem+json; code=not_found when no run has completed", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable or storage_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn stats_get() {}

/// `GET /_auth/login`: begin the browser flow. Redirects to GitHub with
/// the state stored in KV for ten minutes.
#[utoipa::path(
    get,
    path = "/_auth/login",
    responses(
        (status = 302, description = "Location: the GitHub authorize URL; cache-control no-store"),
        (status = 503, description = "problem+json; code=auth_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn auth_login() {}

/// `GET /_auth/callback`: consume the state, exchange the code, gate on
/// org membership, and set the session cookie. The state is single-use:
/// it is deleted before its validity is judged.
#[utoipa::path(
    get,
    path = "/_auth/callback",
    responses(
        (status = 302, description = "Logged in; Location: the configured UI origin, Set-Cookie: the session", ),
        (status = 204, description = "Logged in; Set-Cookie: the session (no UI origin configured)"),
        (status = 400, description = "problem+json; code=malformed_oauth", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "problem+json; code=oauth_state_unknown or unauthorized", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "problem+json; code=forbidden_org", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "problem+json; code=auth_unavailable", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
pub fn auth_callback() {}

/// `POST /logout`: delete the session and expire the cookie. Idempotent.
#[utoipa::path(
    post,
    path = "/logout",
    responses(
        (status = 204, description = "Set-Cookie expires the session cookie; cache-control no-store"),
    )
)]
pub fn auth_logout() {}
