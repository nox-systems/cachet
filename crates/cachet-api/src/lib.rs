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
        description = "A self-hostable nix binary cache on Cloudflare Workers. Writes carry GitHub OIDC credentials; reads carry a GitHub token or the browser session cookie; the public handshake route is unauthenticated. Every route that reads a credential can additionally answer 400 with code=malformed_auth when the Authorization header itself is undecodable.",
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
    ),
    components(schemas(PublicConfig, ProjectList, RenewalBody, ProblemBody, UploadCreated, UploadedPartBody, GcRunList, StatsBody))
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
        };
        let body = serde_json::to_string(&config).expect("serializes");
        assert_eq!(
            body,
            r#"{"oauthClientId":"id","orgs":["org"],"host":"cachet.example.com","publicKey":"cachet.example.com-1:AAAA"}"#,
        );
    }
}
