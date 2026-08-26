//! cachet-api is the HTTP surface, expressed as typed bodies plus the
//! route descriptors the OpenAPI document is derived from (CLAUDE.md §1,
//! docs/openapi.yaml). The code is the source of truth: `just openapi`
//! regenerates docs/openapi.yaml from this crate and CI fails on drift.
//!
//! The response bodies the worker serializes live here as types so the
//! served wire and the published spec share one definition.

#![forbid(unsafe_code)]

pub mod routes;

/// The `GET /api/public/config` body: everything a client needs to reach
/// the deployment's auth and verify its signatures, none of it secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PublicConfig {
    /// The GitHub OAuth App's client id: a public value.
    #[serde(rename = "oauthClientId")]
    pub oauth_client_id: String,
    /// The GitHub orgs this deployment serves.
    pub orgs: Vec<String>,
    /// The deployment's host name, which is also its signing-key name
    /// prefix.
    pub host: String,
    /// The deployment's ed25519 public key in nix's `name:base64` form.
    #[serde(rename = "publicKey")]
    pub public_key: String,
    /// The deployment's name, which is also its stage and its resource
    /// prefix. Housekeeping rather than protocol identity, and the thing
    /// a console names in its header so two tabs are told apart.
    pub deployment: String,
    /// The worker's crate version.
    pub version: String,
    /// The commit the worker was built from, absent when it was built
    /// outside the release path that stamps one.
    #[serde(rename = "buildSha", default, skip_serializing_if = "Option::is_none")]
    pub build_sha: Option<String>,
    /// A stylesheet the console loads for its licensed faces, absent by
    /// default. The repository ships neither the fonts nor an address to
    /// fetch them from; an operator who holds a licence points this at
    /// their own copy.
    #[serde(rename = "fontCss", default, skip_serializing_if = "Option::is_none")]
    pub font_css: Option<String>,
}

/// The `GET /roots` body: the projects currently holding leases.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ProjectList {
    /// The hyphenated `owner-repo` project names.
    pub projects: Vec<String>,
}

impl ProjectList {
    /// Serialize the listing. Stable bytes matter for the same reason the
    /// lease document's do: a diff between two runs must stay legible, so
    /// the keys emit in a fixed shape with a trailing newline.
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string_pretty(self).expect("the listing serializes");
        body.push('\n');
        body
    }
}

/// The `POST /roots/{project}` body: the store paths a project keeps
/// alive plus the installables that produced them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct RenewalBody {
    /// The flake installables the job built.
    pub installables: Vec<String>,
    /// Full nix store paths whose closures must stay alive.
    #[serde(rename = "storePaths")]
    pub store_paths: Vec<String>,
}

/// The `POST /nar/{key}?uploads` body: the new upload's bearer id and the
/// part plan the declaration bound.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UploadCreated {
    /// The upload id: a bearer token naming one in-flight upload.
    #[serde(rename = "uploadId")]
    pub upload_id: String,
    /// How many parts the declared total implies.
    #[serde(rename = "expectedParts")]
    pub expected_parts: u64,
}

/// The `PUT /nar/{key}?uploadId&partNumber` body: the stored part's
/// coordinates, which the completion body echoes back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UploadedPartBody {
    /// The one-based part number just stored.
    #[serde(rename = "partNumber")]
    pub part_number: u16,
    /// The storage layer's part etag.
    pub etag: String,
}

/// `GET /api/self/gc-runs`: one page of run ids, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct GcRunList {
    /// The run ids in chronological order.
    pub runs: Vec<String>,
    /// The cursor for the next page, when more runs exist.
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
}

/// `GET /api/self/stats`: the cache's current shape, derived from the
/// newest completed report rather than recomputed live.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct StatsBody {
    /// The run the numbers came from.
    #[serde(rename = "basedOnRunId")]
    pub based_on_run_id: String,
    /// Narinfos in the bucket at that run's inventory.
    #[serde(rename = "inventoryPaths")]
    pub inventory_paths: u64,
    /// Narinfos its sweep deleted.
    #[serde(rename = "narinfosDeleted")]
    pub narinfos_deleted: u64,
    /// NAR objects its sweep deleted.
    #[serde(rename = "narsDeleted")]
    pub nars_deleted: u64,
    /// Bytes the sweep freed.
    #[serde(rename = "bytesFreed")]
    pub bytes_freed: u64,
    /// The gate the run aborted on, when one tripped.
    pub gate: Option<String>,
    /// When that run finished.
    #[serde(rename = "finishedAtMs")]
    pub finished_at_ms: u64,
}

/// The `POST /api/probe` body: the store-path hashes one run asks about,
/// as 32-character nix base32. One authorized answer per run replaces
/// per-path narinfo HEADs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ProbeBody {
    /// The store-path hash halves, without names or the store directory.
    pub paths: Vec<String>,
}

/// One row of a counter answer: a dimension's value and its totals.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct StatsRow {
    /// The grouped value, empty where the dimension did not apply.
    ///
    /// For `by=hour` and `by=day` this is the bucket's first instant in
    /// epoch seconds, written as digits, so a reader never has to agree
    /// with the SQL engine about a date format.
    pub dimension: String,
    /// How many things it counts, sample-corrected.
    pub count: f64,
    /// Bytes, where the answer is bytes.
    pub bytes: f64,
}

/// What a counter answer was narrowed to, echoed back so a caller can
/// tell a filtered answer from an unfiltered one without re-reading the
/// query string it sent.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
pub struct StatsFilters {
    /// Only this kind of thing, when one was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Only this outcome, when one was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Only this caller class, when one was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

/// The `GET /api/self/health` answer: whether the deployment is keeping
/// up with its own collector, and when it next runs.
///
/// Admin-gated, because it is derived from collection reports. A console
/// showing it to an org member who is not an admin omits it rather than
/// failing: the rest of its header comes from the public config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct HealthBody {
    /// `healthy`, `degraded`, or `unknown`.
    pub status: String,
    /// When the collector fires next, absent when the deployment's cron
    /// is a shape the worker does not recognize.
    #[serde(
        rename = "nextCollectionAtMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_collection_at_ms: Option<u64>,
    /// The run the status was read from, absent before the first one.
    #[serde(
        rename = "latestRunId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub latest_run_id: Option<String>,
    /// When that run finished.
    #[serde(
        rename = "latestFinishedAtMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub latest_finished_at_ms: Option<u64>,
    /// Which gate stopped that run, absent on one that finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
}

/// The `GET /api/self/gc-runs/{runId}` answer: one collection's own
/// record of what it did.
///
/// A mirror of `cachet_core::gc::GcReport`, which the worker streams
/// verbatim from the bucket rather than re-serializing. The mirror
/// exists so the generated document describes the body instead of
/// calling it untyped JSON; a unit test decodes the core type's own
/// bytes into this one, so the two cannot drift apart quietly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct GcReportBody {
    /// The run's identifier, `{millis}-{16 hex}`.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// When the run started.
    #[serde(rename = "startedAtMs")]
    pub started_at_ms: u64,
    /// When it finished.
    #[serde(rename = "finishedAtMs")]
    pub finished_at_ms: u64,
    /// How many paths the inventory held.
    #[serde(rename = "inventoryPaths")]
    pub inventory_paths: u64,
    /// How many leases pinned roots.
    #[serde(rename = "activeLeases")]
    pub active_leases: u64,
    /// How many paths the mark phase reached.
    #[serde(rename = "markedPaths")]
    pub marked_paths: u64,
    /// How many narinfos the walk could not read.
    #[serde(rename = "unreadableDeep")]
    pub unreadable_deep: u64,
    /// How many narinfos the sweep deleted.
    #[serde(rename = "narinfosDeleted")]
    pub narinfos_deleted: u64,
    /// How many NARs the sweep deleted.
    #[serde(rename = "narsDeleted")]
    pub nars_deleted: u64,
    /// How many bytes that freed.
    #[serde(rename = "bytesFreed")]
    pub bytes_freed: u64,
    /// How many abandoned uploads it reaped.
    #[serde(rename = "uploadsAborted")]
    pub uploads_aborted: u64,
    /// Which gate stopped the run, null on a run that finished.
    ///
    /// Null rather than absent, because the core type serializes it that
    /// way and this mirror describes those exact bytes.
    pub gate: Option<String>,
}

/// The `GET /roots/{project}` answer: one lease and what it pins.
///
/// A mirror of `cachet_core::lease::LeaseDocument`, served verbatim from
/// the bucket for the same reason and held in step by the same test.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LeaseBody {
    /// The lease name: the repository with its slash hyphenated.
    pub project: String,
    /// When the lease was last renewed.
    #[serde(rename = "renewedAtMs")]
    pub renewed_at_ms: u64,
    /// `owner/repo` of the run that renewed it.
    pub repository: String,
    /// The ref that run was on.
    #[serde(rename = "ref")]
    pub ref_: String,
    /// That run's id.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// The commit it built.
    #[serde(rename = "commitSha")]
    pub commit_sha: String,
    /// What the run asked for.
    pub installables: Vec<String>,
    /// The store paths the lease pins.
    #[serde(rename = "storePaths")]
    pub store_paths: Vec<String>,
}

/// The `GET /api/whoami` answer: who the caller is to this deployment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct WhoAmI {
    /// The GitHub login the credential resolved to.
    pub login: String,
    /// Whether that login is in `CACHET_ADMINS`.
    pub admin: bool,
    /// Which credential answered: `browser`, `laptop`, or `ci`.
    pub credential: String,
    /// When a browser session stops being accepted. Absent for the other
    /// two, whose lifetimes belong to GitHub and to the issued token's
    /// own record rather than to anything this answer knows.
    #[serde(
        rename = "expiresAtMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at_ms: Option<u64>,
}

/// The `GET /api/self/events` answer: what the chosen question totalled.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct StatsEvents {
    /// What was counted: `reads`, `writes`, or `probes`.
    pub subject: String,
    /// What it was grouped by.
    pub dimension: String,
    /// How far back the answer looks.
    pub window: String,
    /// What the answer was narrowed to.
    pub filters: StatsFilters,
    /// The rows. A dimension list reads largest first; a series reads
    /// oldest first and carries one row per bucket, zeros included.
    pub rows: Vec<StatsRow>,
}

/// The `POST /api/login/exchange` body: what GitHub handed the CLI
/// alongside the access token. Both are optional because an OAuth App
/// that has not opted into expiring tokens issues neither, and its
/// access token then needs no renewing.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
pub struct LoginExchangeBody {
    /// The refresh token, so the deployment can renew without the
    /// person logging in again.
    #[serde(rename = "refreshToken", default)]
    pub refresh_token: String,
    /// Seconds the access token lasts, zero when it does not expire.
    #[serde(rename = "expiresInSeconds", default)]
    pub expires_in_seconds: u64,
}

/// The answer to a read-credential exchange: the token the caller keeps,
/// who it speaks for, and when it stops working.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ReadTokenIssued {
    /// The credential. Shown once; the deployment stores only its hash.
    pub token: String,
    /// The GitHub login it speaks for.
    pub login: String,
    /// When it stops being accepted, epoch milliseconds.
    #[serde(rename = "expiresAtMs")]
    pub expires_at_ms: u64,
}

/// The `POST /api/probe` answer: the hashes with a narinfo stored,
/// ascending. A narinfo present implies its NAR (never-dangle), so one
/// list is the whole answer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ProbeAnswer {
    /// The hashes the bucket holds, sorted.
    pub present: Vec<String>,
}

/// The RFC 9457 problem document every non-2xx answer carries. Described
/// here rather than serialized from it: the worker emits cachet-core's
/// problem writer, whose byte shape is golden-locked, and this type is
/// the document's schema for readers.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ProblemBody {
    /// Always `about:blank`.
    #[serde(rename = "type")]
    pub type_: String,
    /// The HTTP status, repeated in the body.
    pub status: u16,
    /// The code in words.
    pub title: String,
    /// cachet's stable machine code, from the error-code table.
    pub code: String,
}

/// The whole surface, derived. Path order is the router's: cache reads
/// first, then discovery, then the lease routes, the write space, and
/// finally the browser login flow.
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "cachet",
        version = env!("CARGO_PKG_VERSION"),
        description = "A self-hostable nix binary cache on Cloudflare Workers. Writes carry GitHub OIDC credentials; reads carry a credential this deployment issued, a CI job's OIDC token, or the browser session cookie; the public handshake route is unauthenticated. Every route that reads a credential can additionally answer 400 with code=malformed_auth when the Authorization header itself is undecodable.",
    ),
    paths(
        routes::cache_info_get,
        routes::cache_info_head,
        routes::narinfo_get,
        routes::narinfo_head,
        routes::nar_get,
        routes::nar_head,
        routes::public_config_get,
        routes::openapi_get,
        routes::probe_post,
        routes::login_exchange,
        routes::login_revoke,
        routes::projects_list,
        routes::lease_get,
        routes::lease_renew,
        routes::narinfo_put,
        routes::nar_put,
        routes::upload_create,
        routes::upload_part_put,
        routes::upload_complete,
        routes::upload_abort,
        routes::auth_login,
        routes::auth_callback,
        routes::auth_logout,
        routes::gc_runs_list,
        routes::gc_run_get,
        routes::stats_get,
        routes::stats_events,
        routes::whoami,
        routes::health,
    ),
    components(schemas(PublicConfig, ProjectList, RenewalBody, ProbeBody, ProbeAnswer, ProblemBody, UploadCreated, UploadedPartBody, GcRunList, StatsBody, ReadTokenIssued, LoginExchangeBody, StatsRow, StatsFilters, StatsEvents, WhoAmI, GcReportBody, LeaseBody, HealthBody))
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_list_serializes_stably() {
        let list = ProjectList {
            projects: vec!["my-org-my-repo".to_string()],
        };
        assert_eq!(
            list.serialize(),
            "{\n  \"projects\": [\n    \"my-org-my-repo\"\n  ]\n}\n",
        );
        assert_eq!(
            ProjectList { projects: vec![] }.serialize(),
            "{\n  \"projects\": []\n}\n"
        );
    }

    #[test]
    fn the_public_config_field_names_are_the_wire_names() {
        let config = PublicConfig {
            oauth_client_id: "id".to_string(),
            orgs: vec!["org".to_string()],
            host: "cachet.example.com".to_string(),
            public_key: "cachet.example.com-1:AAAA".to_string(),
            deployment: "production".to_string(),
            version: "0.1.0".to_string(),
            build_sha: None,
            font_css: None,
        };
        let body = serde_json::to_string(&config).expect("serializes");
        assert_eq!(
            body,
            r#"{"oauthClientId":"id","orgs":["org"],"host":"cachet.example.com","publicKey":"cachet.example.com-1:AAAA","deployment":"production","version":"0.1.0"}"#,
        );
        // The two optional fields are absent rather than null, so a
        // deployment that stamps no commit and licenses no fonts serves
        // the same document it always did plus its identity.
        let stamped = PublicConfig {
            build_sha: Some("a4f31c".to_string()),
            font_css: Some("https://fonts.example.com/cachet.css".to_string()),
            ..config
        };
        let body = serde_json::to_string(&stamped).expect("serializes");
        assert!(body.contains(r#""buildSha":"a4f31c""#), "{body}");
        assert!(
            body.contains(r#""fontCss":"https://fonts.example.com/cachet.css""#),
            "{body}"
        );
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::{GcReportBody, LeaseBody};

    /// The two bodies the worker streams verbatim are described in the
    /// generated document by mirrors, and a mirror that drifted would
    /// document a shape the deployment does not serve. Decoding the core
    /// type's own serialization into the mirror is what stops that: a
    /// renamed or dropped field fails here rather than in a client.
    #[test]
    fn the_mirrors_decode_what_the_worker_actually_serves() {
        let report = cachet_core::gc::GcReport {
            run_id: "1780000000000-0123456789abcdef".to_string(),
            started_at_ms: 1_780_000_000_000,
            finished_at_ms: 1_780_000_012_345,
            inventory_paths: 4_213,
            active_leases: 7,
            marked_paths: 4_102,
            unreadable_deep: 0,
            narinfos_deleted: 111,
            nars_deleted: 98,
            bytes_freed: 8_123_456_789,
            uploads_aborted: 2,
            gate: Some("sweep_fraction_exceeded".to_string()),
        };
        // why: the bytes, not the fields. A mirror can decode a body
        // correctly and still describe it wrongly, which is exactly what
        // a skip_serializing_if on a field the core type writes as null
        // does: the document then promises an absent key for a key that
        // is always present. Comparing serializations catches that.
        let served = serde_json::to_string(&report).expect("the report serializes");
        let mirrored: GcReportBody = serde_json::from_str(&served).expect("the mirror decodes it");
        assert_eq!(
            serde_json::to_value(&mirrored).expect("the mirror serializes"),
            serde_json::to_value(&report).expect("the report serializes"),
            "the mirror describes the bytes the worker serves"
        );
        // A tripped gate and a clean run are different bytes, and both
        // have to round-trip.
        let clean = cachet_core::gc::GcReport {
            gate: None,
            ..report.clone()
        };
        let mirrored_clean: GcReportBody =
            serde_json::from_str(&serde_json::to_string(&clean).expect("serializes"))
                .expect("the mirror decodes a clean run");
        assert_eq!(
            serde_json::to_value(&mirrored_clean).expect("serializes"),
            serde_json::to_value(&clean).expect("serializes"),
        );

        let lease = cachet_core::lease::LeaseDocument {
            project: "nox-systems-cachet".to_string(),
            renewed_at_ms: 1_780_000_000_000,
            repository: "nox-systems/cachet".to_string(),
            ref_: "refs/heads/main".to_string(),
            run_id: "123".to_string(),
            commit_sha: "abc".to_string(),
            installables: vec![".#devShells.aarch64-darwin.default".to_string()],
            store_paths: vec!["/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2".to_string()],
        };
        let served = serde_json::to_string(&lease).expect("the lease serializes");
        let mirrored: LeaseBody = serde_json::from_str(&served).expect("the mirror decodes it");
        assert_eq!(
            serde_json::to_value(&mirrored).expect("the mirror serializes"),
            serde_json::to_value(&lease).expect("the lease serializes"),
            "the mirror describes the bytes the worker serves"
        );
        // The lease's own reader is a hand parser rather than serde, so
        // the round trip goes through that instead.
        let round = cachet_core::lease::LeaseDocument::parse(
            &serde_json::to_string(&mirrored).expect("the mirror serializes"),
        )
        .expect("the lease parser reads the mirror");
        assert_eq!(round, lease);
    }
}
