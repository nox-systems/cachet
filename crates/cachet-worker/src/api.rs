//! `/api/public/config`: the one unauthenticated JSON document. The CLI
//! and the later SPA discover the deployment's OAuth client id, orgs,
//! host, and public signing key here, so nothing the client needs ever
//! rides in a secret channel it does not yet have.

use cachet_core::constants::{
    ACCOUNT_ID_VAR, DEPLOY_NAME_VAR, FONT_CSS_VAR, GC_CRON_VAR, GC_LATEST_REPORT_KEY,
    GC_REPORTS_KEY_PREFIX, GC_RUNS_PAGE_LIMIT, STATS_API_DEFAULT, STATS_API_URL_VAR,
    STATS_DATASET_VAR, STATS_TOKEN_SECRET,
};
use cachet_core::error::ClientError;
use cachet_core::gc::{GcReport, parse_run_id};
use cachet_core::schedule::DailySchedule;
use cachet_core::types::UnixMillis;
use cachet_crypto::ed25519::NixSecretKey;
use worker::{Env, Request, Response};

use crate::{auth, log, verdict};

/// Serve the public configuration. The signing key's public half is
/// computed from the secret binding: the deployment answers from its own
/// custody rather than from a second copy that could drift.
pub fn public_config(env: &Env) -> worker::Result<Response> {
    let config = match auth::oidc_config(env) {
        Ok(config) => config,
        Err(code) => return crate::error::problem_response(code),
    };
    let client_id = match env.var("CACHET_OAUTH_CLIENT_ID") {
        Ok(client_id) => client_id.to_string(),
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    let host = match env.var("CACHET_HOST") {
        Ok(host) => host.to_string(),
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    let public_key = match env.secret("CACHET_SIGNING_KEY") {
        Ok(secret) => match NixSecretKey::parse(&secret.to_string()) {
            Ok(key) => key.public_key_text(),
            Err(failure) => {
                crate::log::event(
                    "error",
                    "api.signing_key_corrupt",
                    &[("error", format!("{failure:?}"))],
                );
                return crate::error::problem_response(ClientError::AuthUnavailable);
            }
        },
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    log::event("info", "api.public_config", &[]);
    // The body is cachet-api's shared type: the served wire and the
    // published OpenAPI schema cannot drift apart here.
    let body = serde_json::to_string(&cachet_api::PublicConfig {
        oauth_client_id: client_id,
        orgs: config.orgs,
        host,
        public_key,
        deployment: env
            .var(DEPLOY_NAME_VAR)
            .map_or_else(|_| "cachet".to_string(), |value| value.to_string()),
        version: env!("CARGO_PKG_VERSION").to_string(),
        // why: stamped by the build that produces the deployable bundle,
        // absent from any other build. A worker compiled outside that
        // path says nothing here rather than naming a commit it was not
        // built from.
        build_sha: option_env!("CACHET_BUILD_SHA")
            .filter(|sha| !sha.is_empty())
            .map(ToString::to_string),
        font_css: env
            .var(FONT_CSS_VAR)
            .ok()
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty()),
    })
    .expect("the config fields serialize");
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::ok(body)?.with_headers(headers))
}

/// `/api/openapi.json`: the committed generated document, served verbatim
/// so the drift gate and the served bytes check the same artifact.
pub fn openapi_document() -> worker::Result<Response> {
    let headers = worker::Headers::new();
    headers.set("content-type", "application/yaml")?;
    headers.set("cache-control", "public, max-age=300")?;
    Ok(Response::ok(OPENAPI_YAML)?.with_headers(headers))
}

// why: serving reads the same committed file the drift gate regenerates,
// so a spec change that forgets to regenerate fails CI, not clients.
const OPENAPI_YAML: &str = include_str!("../../../docs/openapi.yaml");

/// JSON with no caching: every `/api/self` answer rides live state.
fn self_headers() -> worker::Result<worker::Headers> {
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(headers)
}

/// A JSON body no cache may keep: every credential answer wears this.
///
/// # Errors
///
/// Propagates a header or body failure as the worker's generic 500.
pub fn json_no_store<B: serde::Serialize>(body: &B) -> worker::Result<Response> {
    let text = serde_json::to_string(body).expect("typed bodies serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// `GET /api/self/events`: the deployment's own counters.
///
/// The caller chooses a question; the worker composes the SQL. That is
/// the whole security posture of this route, because the credential
/// behind it is a Cloudflare API token: a caller who could compose SQL
/// would be composing it with that token's authority. Every part of the
/// statement is a literal or an enum value (cachet-core's
/// `stats_query`), so no caller text reaches it at all.
///
/// # Errors
///
/// Propagates a header or body failure as the worker's generic 500.
pub async fn stats_events(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    let url = req.url()?;
    let pick = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let Some(query) = compose_query(&pick) else {
        return crate::error::problem_response(ClientError::MalformedQuery);
    };

    let Ok(token) = env.secret(STATS_TOKEN_SECRET) else {
        // A deployment that has not been given the token counts happily
        // and simply cannot report; that is configuration, not an
        // outage the caller caused.
        log::event("warn", "api.stats_token_missing", &[]);
        return crate::error::problem_response(ClientError::StorageUnavailable);
    };
    let Ok(account) = env.var(ACCOUNT_ID_VAR) else {
        log::event("warn", "api.stats_account_missing", &[]);
        return crate::error::problem_response(ClientError::StorageUnavailable);
    };
    let dataset = env
        .var(STATS_DATASET_VAR)
        .map(|value| value.to_string())
        .unwrap_or_default();
    if dataset.is_empty() {
        log::event("warn", "api.stats_dataset_missing", &[]);
        return crate::error::problem_response(ClientError::StorageUnavailable);
    }

    let base = env
        .var(STATS_API_URL_VAR)
        .map_or_else(|_| STATS_API_DEFAULT.to_string(), |value| value.to_string());

    let rows = match run_stats_sql(
        &base,
        &account.to_string(),
        &token.to_string(),
        &query.sql(&dataset),
    )
    .await
    {
        Ok(rows) => rows,
        Err(code) => return crate::error::problem_response(code),
    };
    let body = cachet_api::StatsEvents {
        subject: query.subject.name().to_string(),
        dimension: query.dimension.name().to_string(),
        window: query.window.name().to_string(),
        filters: cachet_api::StatsFilters {
            kind: query.filters.kind.map(|kind| kind.name().to_string()),
            outcome: query
                .filters
                .outcome
                .map(|outcome| outcome.render().into_owned()),
            actor: query.filters.actor.map(|actor| actor.name().to_string()),
        },
        rows: shape_rows(&query, now, rows),
    };
    let text = serde_json::to_string(&body).expect("typed bodies serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// The collector's own record of its most recent run, if it has one.
///
/// Both `/api/self/stats` and `/api/self/health` are projections of this
/// object, and they differ in what an absent one means: stats has
/// nothing to project and answers 404, health answers `unknown`, which
/// is a status.
async fn read_latest_report(env: &Env) -> std::result::Result<Option<GcReport>, ClientError> {
    let bucket = env
        .bucket("CACHE_BUCKET")
        .map_err(|_| ClientError::StorageUnavailable)?;
    let object = match bucket.get(GC_LATEST_REPORT_KEY).execute().await {
        Ok(object) => object,
        Err(failure) => {
            log::event(
                "error",
                "api.stats_read_failed",
                &[("error", failure.to_string())],
            );
            return Err(ClientError::StorageUnavailable);
        }
    };
    let Some(object) = object else {
        return Ok(None);
    };
    let Some(body) = object.body() else {
        return Err(ClientError::StorageUnavailable);
    };
    let text = body
        .text()
        .await
        .map_err(|_| ClientError::StorageUnavailable)?;
    match GcReport::parse(&text) {
        Ok(report) => Ok(Some(report)),
        Err(failure) => {
            log::alert("api.latest_report_corrupt");
            log::event(
                "error",
                "api.stats_parse_failed",
                &[("error", format!("{failure:?}"))],
            );
            Err(ClientError::StorageUnavailable)
        }
    }
}

/// `GET /api/self/health`: whether the collector is keeping up, and when
/// it runs next.
///
/// Derived from the same latest-report object `/api/self/stats` reads,
/// plus the cron the deployment was created with. A deployment that has
/// never collected answers `unknown` rather than 404, because a console
/// header renders on every screen and a missing header reads as a broken
/// console where "no run yet" reads as a young deployment.
///
/// # Errors
///
/// Propagates a header or body failure as the worker's generic 500.
pub async fn health(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    let next_collection_at_ms = env
        .var(GC_CRON_VAR)
        .ok()
        .and_then(|value| DailySchedule::parse(&value.to_string()))
        .map(|schedule| schedule.next_after_ms(now.as_u64()));

    let latest = match read_latest_report(env).await {
        Ok(report) => report,
        Err(code) => return crate::error::problem_response(code),
    };
    let body = match latest {
        None => cachet_api::HealthBody {
            status: "unknown".to_string(),
            next_collection_at_ms,
            latest_run_id: None,
            latest_finished_at_ms: None,
            gate: None,
        },
        Some(report) => {
            // why: two cron periods. One missed run is a deploy window or
            // a platform hiccup; two means the schedule is not firing,
            // which is the thing an operator wants a colour for.
            let stale = now.saturating_ms_since(UnixMillis::new(report.finished_at_ms))
                > 2 * cachet_core::constants::MILLIS_PER_DAY;
            let healthy = report.gate.is_none() && !stale;
            cachet_api::HealthBody {
                status: if healthy { "healthy" } else { "degraded" }.to_string(),
                next_collection_at_ms,
                latest_run_id: Some(report.run_id),
                latest_finished_at_ms: Some(report.finished_at_ms),
                gate: report.gate,
            }
        }
    };
    let text = serde_json::to_string(&body).expect("typed bodies serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// `GET /api/whoami`: who this request authenticates as.
///
/// Any read credential resolves here, admin or not, because the console
/// asks this before it renders anything and an org member who is not an
/// admin still gets an answer: their own login, and `admin: false`. The
/// alternative was a console that learns its standing by provoking a
/// 403, which makes a refusal a normal part of loading a page and hides
/// the real ones.
///
/// # Errors
///
/// Propagates a header or body failure as the worker's generic 500.
pub async fn whoami(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    let identity = match verdict::authorize_read(env, now, req).await {
        Ok(identity) => identity,
        Err(code) => return crate::error::problem_response(code),
    };
    let login = identity.login().to_string();
    let body = cachet_api::WhoAmI {
        admin: verdict::admins(env).iter().any(|admin| admin == &login),
        credential: identity.credential().to_string(),
        expires_at_ms: match identity {
            verdict::ReadIdentity::Session { expires_at_ms, .. } => Some(expires_at_ms),
            _ => None,
        },
        login,
    };
    let text = serde_json::to_string(&body).expect("typed bodies serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// Turn a caller's choices into one question, or into nothing.
///
/// Every parameter parses into a closed enum or fails, and the assembled
/// pair is checked too: a bucket finer than its window can hold is a
/// question with no admissible answer rather than a truncated one.
fn compose_query(
    pick: &impl Fn(&str) -> Option<String>,
) -> Option<cachet_core::stats_query::StatsQuery> {
    use cachet_core::stats::{StatActor, StatKind, StatOutcome};
    use cachet_core::stats_query::{QueryDimension, QueryFilters, QuerySubject, QueryWindow};

    let subject = QuerySubject::parse(&pick("subject")?)?;
    let dimension = QueryDimension::parse(&pick("by").unwrap_or_else(|| "outcome".to_string()))?;
    let window = QueryWindow::parse(pick("window").as_deref())?;
    // An unstated filter is absent; a stated one that names nothing is a
    // refusal, so a typo narrows to nothing loudly instead of quietly
    // answering the unfiltered question.
    let filters = QueryFilters {
        kind: match pick("kind") {
            None => None,
            Some(text) => Some(StatKind::parse(&text)?),
        },
        outcome: match pick("outcome") {
            None => None,
            Some(text) => Some(StatOutcome::parse(&text)?),
        },
        actor: match pick("actor") {
            None => None,
            Some(text) => Some(StatActor::parse(&text)?),
        },
    };
    cachet_core::stats_query::StatsQuery::new(subject, dimension, window, filters)
}

/// Answer with the rows the question implies.
///
/// A dimension list passes through: the statement already ordered and
/// bounded it. A series is filled to one row per bucket, because
/// Analytics Engine returns nothing for a bucket nothing happened in and
/// a line drawn through the holes claims traffic was smooth when it was
/// absent.
fn shape_rows(
    query: &cachet_core::stats_query::StatsQuery,
    now: UnixMillis,
    rows: Vec<cachet_api::StatsRow>,
) -> Vec<cachet_api::StatsRow> {
    use cachet_core::stats_query::{SeriesPoint, fill_series};
    if query.bucket_count().is_none() {
        return rows;
    }
    let observed: Vec<SeriesPoint> = rows
        .iter()
        .filter_map(|row| {
            Some(SeriesPoint {
                start_secs: row.dimension.parse().ok()?,
                count: row.count,
                bytes: row.bytes,
            })
        })
        .collect();
    fill_series(query, now.as_u64(), &observed)
        .into_iter()
        .map(|point| cachet_api::StatsRow {
            dimension: point.start_secs.to_string(),
            count: point.count,
            bytes: point.bytes,
        })
        .collect()
}

/// Run one composed statement against Cloudflare's SQL API.
async fn run_stats_sql(
    base: &str,
    account: &str,
    token: &str,
    sql: &str,
) -> std::result::Result<Vec<cachet_api::StatsRow>, ClientError> {
    let headers = worker::Headers::new();
    let _ = headers.set("authorization", &format!("Bearer {token}"));
    let _ = headers.set("content-type", "text/plain");
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Post);
    init.headers.clone_from(&headers);
    init.with_body(Some(sql.to_string().into()));
    let request = worker::Request::new_with_init(
        &format!("{base}/accounts/{account}/analytics_engine/sql"),
        &init,
    )
    .map_err(|_| ClientError::StorageUnavailable)?;
    let mut response = worker::Fetch::Request(request)
        .send()
        .await
        .map_err(|_| ClientError::StorageUnavailable)?;
    if response.status_code() != 200 {
        // why: the upstream's own words never reach the caller. It
        // answers about an account, not about this cache, and an admin
        // reading a chart is not the audience for a Cloudflare error.
        log::event(
            "error",
            "api.stats_query_failed",
            &[("status", response.status_code().to_string())],
        );
        return Err(ClientError::StorageUnavailable);
    }
    let answer: SqlAnswer = response
        .json()
        .await
        .map_err(|_| ClientError::StorageUnavailable)?;
    Ok(answer
        .data
        .into_iter()
        .map(|row| cachet_api::StatsRow {
            dimension: row.dimension(),
            count: row.count,
            bytes: row.bytes,
        })
        .collect())
}

/// Cloudflare's SQL answer, only the part this route reads.
#[derive(serde::Deserialize)]
struct SqlAnswer {
    data: Vec<SqlRow>,
}

/// One row, named by the aliases `stats_query` gives its columns.
///
/// `dimension` arrives as text for a blob column and as a number for a
/// time bucket, because Analytics Engine offers no cast to text and a
/// statement asking for one is rejected. Reading it as a JSON value and
/// rendering it here keeps the wire contract one shape: the answer's
/// dimension is always a string.
#[derive(serde::Deserialize)]
struct SqlRow {
    dimension: serde_json::Value,
    #[serde(default)]
    count: f64,
    #[serde(default)]
    bytes: f64,
}

impl SqlRow {
    /// The dimension as the answer states it.
    fn dimension(&self) -> String {
        match &self.dimension {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Number(number) => number.to_string(),
            other => other.to_string(),
        }
    }
}

/// `GET /api/self/gc-runs`: one page of run ids, oldest first. Pagination
/// follows the bucket's own cursor, so a deep history page-turns cheaply.
pub async fn gc_runs_list(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;
    let query_cursor = req
        .url()?
        .query_pairs()
        .find(|(key, _)| key == "cursor")
        .map(|(_, value)| value.to_string());
    let page = u32::try_from(GC_RUNS_PAGE_LIMIT).expect("the page limit fits u32");
    let mut builder = bucket
        .list()
        .prefix(GC_REPORTS_KEY_PREFIX.to_string())
        .limit(page);
    if let Some(cursor) = query_cursor {
        builder = builder.cursor(cursor);
    }
    let listed = match builder.execute().await {
        Ok(listed) => listed,
        Err(failure) => {
            log::event(
                "error",
                "api.gc_runs_list_failed",
                &[("error", failure.to_string())],
            );
            return crate::error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let mut runs: Vec<String> = listed
        .objects()
        .iter()
        .filter_map(|object| {
            let key = object.key();
            let suffix = key.strip_prefix(GC_REPORTS_KEY_PREFIX)?;
            let run_id = suffix.strip_suffix(".json")?;
            // The index copy is not a run.
            (run_id != "latest").then(|| run_id.to_string())
        })
        .collect();
    runs.sort();
    let body = cachet_api::GcRunList {
        runs,
        next_cursor: listed.truncated().then(|| listed.cursor()).flatten(),
    };
    let text = serde_json::to_string(&body).expect("run ids serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// `GET /api/self/gc-runs/{runId}`: one run's report, served as stored.
pub async fn gc_run_read(
    env: &Env,
    now: UnixMillis,
    req: &Request,
    run_id: &str,
) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    if let Err(code) = parse_run_id(run_id) {
        return crate::error::problem_response(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;
    let key = format!("{GC_REPORTS_KEY_PREFIX}{run_id}.json");
    let object = match bucket.get(&key).execute().await {
        Ok(object) => object,
        Err(failure) => {
            log::event(
                "error",
                "api.gc_run_read_failed",
                &[("error", failure.to_string())],
            );
            return crate::error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let Some(object) = object else {
        return crate::error::problem_response(ClientError::NotFound);
    };
    let Some(body) = object.body() else {
        return crate::error::problem_response(ClientError::StorageUnavailable);
    };
    let text = body.text().await?;
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// `GET /api/self/stats`: the cache's totals from the newest completed
/// report, written by the collector itself so the answer is one read.
pub async fn stats(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    let report = match read_latest_report(env).await {
        Ok(Some(report)) => report,
        // why: a fresh deployment has no report yet; the empty answer is a
        // fact, and 404 is its honest shape rather than fabricated zeros.
        Ok(None) => return crate::error::problem_response(ClientError::NotFound),
        Err(code) => return crate::error::problem_response(code),
    };
    let body = cachet_api::StatsBody {
        based_on_run_id: report.run_id,
        inventory_paths: report.inventory_paths,
        narinfos_deleted: report.narinfos_deleted,
        nars_deleted: report.nars_deleted,
        bytes_freed: report.bytes_freed,
        gate: report.gate,
        finished_at_ms: report.finished_at_ms,
    };
    let text = serde_json::to_string(&body).expect("the stats fields serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}
