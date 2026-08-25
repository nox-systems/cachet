//! The scripted end-to-end evidence: fake adapters answer, the pipeline
//! drives, and the request log answers what the wire saw. These tests are
//! the unit lane's analog of the workerd lane for the write path's other
//! half.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use crate::adapters::{Adapters, Commands, Http, TokenSource, WireAnswer};
use crate::error::PushError;
use crate::pipeline::{PushEvent, PushInputs, Sleeper, push};

/// A scripted nix+fs adapter. Its files key by their staging-relative
/// path (`nar/<file>.nar.zst`, `<hash>.narinfo`), exactly as the real
/// tree lays them out: keying by basename would flatten the `nar/`
/// level and could never see a wrong join upstream.
#[derive(Default)]
struct FakeCommands {
    path_info_all: Mutex<VecDeque<Result<String, PushError>>>,
    path_info: Mutex<BTreeMap<String, Result<String, PushError>>>,
    copy_log: Mutex<Vec<(String, Vec<String>)>>,
    layout: Mutex<Vec<(String, u64)>>,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    staging: Mutex<Option<std::path::PathBuf>>,
}

impl FakeCommands {
    fn with_store(after: &str) -> Self {
        let fake = Self::default();
        fake.path_info_all
            .lock()
            .expect("answers queue")
            .push_back(Ok(after.to_string()));
        fake
    }

    /// The files map's key for a path the pipeline opened: the path
    /// relative to the staging directory read_dir recorded. A read that
    /// escapes the staging tree fails the script loudly.
    fn staged_key(&self, path: &std::path::Path) -> String {
        let staging = self
            .staging
            .lock()
            .expect("staging")
            .clone()
            .expect("read_dir ran before any file read");
        path.strip_prefix(&staging)
            .expect("a read inside the staging tree")
            .to_str()
            .expect("utf8 paths")
            .to_string()
    }
}

impl Commands for FakeCommands {
    async fn path_info_all(&self) -> Result<String, PushError> {
        self.path_info_all
            .lock()
            .expect("answers queue")
            .pop_front()
            .unwrap_or_else(|| Ok(String::new()))
    }
    async fn path_info(&self, installable: &str) -> Result<String, PushError> {
        self.path_info
            .lock()
            .expect("answers map")
            .get(installable)
            .cloned()
            .unwrap_or(Err(PushError::Detail {
                message: format!("no scripted answer for {installable}"),
            }))
    }
    async fn copy_to(&self, destination: &str, paths: &[String]) -> Result<(), PushError> {
        self.copy_log
            .lock()
            .expect("copy log")
            .push((destination.to_string(), paths.to_vec()));
        Ok(())
    }
    async fn read_dir(&self, dir: &std::path::Path) -> Result<Vec<(String, u64)>, PushError> {
        *self.staging.lock().expect("staging") = Some(dir.to_path_buf());
        Ok(self.layout.lock().expect("layout").clone())
    }
    async fn read_file(&self, path: &std::path::Path) -> Result<Vec<u8>, PushError> {
        self.files
            .lock()
            .expect("files")
            .get(self.staged_key(path).as_str())
            .cloned()
            .ok_or(PushError::Detail {
                message: format!("no scripted file for {}", path.display()),
            })
    }
    async fn read_range(
        &self,
        path: &std::path::Path,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, PushError> {
        let files = self.files.lock().expect("files");
        let key = self.staged_key(path);
        let bytes = files.get(key.as_str()).ok_or(PushError::Detail {
            message: format!("no scripted file for {}", path.display()),
        })?;
        let (offset, len) = (
            usize::try_from(offset).expect("fit"),
            usize::try_from(len).expect("fit"),
        );
        Ok(bytes
            .get(offset..offset + len)
            .ok_or(PushError::Detail {
                message: format!("range {offset}..+{len} past {}", path.display()),
            })?
            .to_vec())
    }
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    method: String,
    url: String,
    bearer: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// An answering-in-order wire.
#[derive(Default)]
struct FakeHttp {
    head_answers: Mutex<BTreeMap<String, VecDeque<u16>>>,
    post_answers: Mutex<BTreeMap<String, VecDeque<Result<WireAnswer, PushError>>>>,
    put_answers: Mutex<BTreeMap<String, VecDeque<Result<WireAnswer, PushError>>>>,
    delete_log: Mutex<Vec<String>>,
    calls: Mutex<Vec<Call>>,
}

impl FakeHttp {
    fn post(&self, url: &str, status: u16, body: &str) {
        self.post_answers
            .lock()
            .expect("answers")
            .entry(url.to_string())
            .or_default()
            .push_back(Ok(WireAnswer {
                status,
                body: body.as_bytes().to_vec(),
            }));
    }
    fn put(&self, url: &str, status: u16, body: &str) {
        self.put_answers
            .lock()
            .expect("answers")
            .entry(url.to_string())
            .or_default()
            .push_back(Ok(WireAnswer {
                status,
                body: body.as_bytes().to_vec(),
            }));
    }
    fn fail_put(&self, url: &str, message: &str) {
        self.put_answers
            .lock()
            .expect("answers")
            .entry(url.to_string())
            .or_default()
            .push_back(Err(PushError::Detail {
                message: message.to_string(),
            }));
    }
    fn fail_post(&self, url: &str, message: &str) {
        self.post_answers
            .lock()
            .expect("answers")
            .entry(url.to_string())
            .or_default()
            .push_back(Err(PushError::Detail {
                message: message.to_string(),
            }));
    }
    fn record(&self, call: Call) {
        self.calls.lock().expect("calls").push(call);
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }
}

impl Http for FakeHttp {
    async fn head(&self, url: &str, bearer: Option<&str>) -> Result<u16, PushError> {
        self.record(Call {
            method: "HEAD".to_string(),
            url: url.to_string(),
            bearer: bearer.map(str::to_string),
            headers: Vec::new(),
            body: Vec::new(),
        });
        self.head_answers
            .lock()
            .expect("answers")
            .get_mut(url)
            .and_then(VecDeque::pop_front)
            .ok_or(PushError::Detail {
                message: format!("no scripted HEAD answer for {url}"),
            })
    }
    async fn get(&self, url: &str, bearer: Option<&str>) -> Result<WireAnswer, PushError> {
        Err(PushError::Detail {
            message: format!("unscripted GET {url} (bearer {bearer:?})"),
        })
    }
    async fn put(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<WireAnswer, PushError> {
        self.record(Call {
            method: "PUT".to_string(),
            url: url.to_string(),
            bearer: Some(bearer.to_string()),
            headers: headers.to_vec(),
            body,
        });
        self.put_answers
            .lock()
            .expect("answers")
            .get_mut(url)
            .and_then(VecDeque::pop_front)
            .ok_or(PushError::Detail {
                message: format!("no scripted PUT answer for {url}"),
            })?
    }
    async fn post(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<WireAnswer, PushError> {
        self.record(Call {
            method: "POST".to_string(),
            url: url.to_string(),
            bearer: Some(bearer.to_string()),
            headers: headers.to_vec(),
            body,
        });
        self.post_answers
            .lock()
            .expect("answers")
            .get_mut(url)
            .and_then(VecDeque::pop_front)
            .ok_or(PushError::Detail {
                message: format!("no scripted POST answer for {url}"),
            })?
    }
    async fn delete(&self, url: &str, bearer: &str) -> Result<u16, PushError> {
        self.delete_log
            .lock()
            .expect("log")
            .push(format!("{url} as {bearer}"));
        Ok(204)
    }
}

/// Tokens mint in sequence so refreshes are visible in the log.
#[derive(Default)]
struct FakeTokens {
    counter: Mutex<u64>,
}

impl FakeTokens {
    fn mints(&self) -> u64 {
        *self.counter.lock().expect("counter")
    }
}

impl TokenSource for FakeTokens {
    async fn mint(&self, audience: &str) -> Result<String, PushError> {
        assert_eq!(audience, "cachet-test");
        let mut counter = self.counter.lock().expect("counter");
        *counter += 1;
        Ok(format!("token-{}", *counter))
    }
}

fn no_sleep() -> Box<Sleeper> {
    Box::new(|_ms| Box::pin(async {}))
}

fn collect_events() -> (std::sync::Arc<Mutex<Vec<String>>>, impl FnMut(PushEvent)) {
    let log = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let log = std::sync::Arc::clone(&log);
        move |event: PushEvent| {
            log.lock().expect("event log").push(format!("{event:?}"));
        }
    };
    (log, sink)
}

const BUILT: &str = "/nix/store/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq-built-1";
const ROOTED: &str = "/nix/store/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr-leafroot";
/// The fixture narinfo's key: the built path's hash half names it.
const NARINFO_KEY: &str = "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo";

/// The bulk probe's answer: one scripted POST body naming the held hash
/// halves, replacing the per-path HEAD scripts of the old two-pass world.
fn stub_probe(http: &FakeHttp, present: &[&str]) {
    let hashes = present
        .iter()
        .map(|hash| format!("\"{hash}\""))
        .collect::<Vec<_>>()
        .join(",");
    http.post(
        "https://cachet.test/api/probe",
        200,
        &format!("{{\"present\":[{hashes}]}}"),
    );
}

/// The bulk probe answers an empty bucket.
fn stub_probe_absent(http: &FakeHttp) {
    stub_probe(http, &[]);
}

/// The scripted survivor's own narinfo body: parseable and naming the
/// staged NAR's key. The pipeline now reads the pair's ownership out of
/// the staged narinfo itself, so these fixtures must parse for real.
fn survivor_body(store_path: &str, nar_key: &str) -> String {
    format!(
        "StorePath: {store_path}\nURL: {nar_key}\nCompression: zstd\nNarHash: sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j\nNarSize: 160\n"
    )
}

/// Seed the survivor pair into the fake staging tree: layout rows and
/// file bodies for the narinfo and the NAR it names.
fn seed_pair(
    commands: &FakeCommands,
    narinfo_key: &str,
    narinfo_body: String,
    nar_key: &str,
    nar_bytes: Vec<u8>,
) {
    let nar_size = nar_bytes.len() as u64;
    commands.layout.lock().expect("layout").extend([
        (narinfo_key.to_string(), narinfo_body.len() as u64),
        (nar_key.to_string(), nar_size),
    ]);
    commands.files.lock().expect("files").extend([
        (narinfo_key.to_string(), narinfo_body.into_bytes()),
        (nar_key.to_string(), nar_bytes),
    ]);
}

/// Stage a NAR of `size` bytes under the fixture content hash and answer
/// with its object key.
fn stage_big_nar(commands: &FakeCommands, size: u64) -> String {
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    commands
        .layout
        .lock()
        .expect("layout")
        .extend([(nar_key.clone(), size)]);
    commands.files.lock().expect("files").insert(
        nar_key.clone(),
        vec![b'z'; usize::try_from(size).expect("fits memory")],
    );
    nar_key
}

/// A successful quartet: the begin answer, one ok PUT per part, the
/// completion POST.
fn stub_quartet_ok(http: &FakeHttp, nar_key: &str, parts: u16) {
    http.post(
        &format!("https://cachet.test/{nar_key}?uploads"),
        200,
        &format!(r#"{{"uploadId":"up-1","expectedParts":{parts}}}"#),
    );
    for number in 1..=parts {
        http.put(
            &format!("https://cachet.test/{nar_key}?uploadId=up-1&partNumber={number}"),
            200,
            &format!(r#"{{"partNumber":{number},"etag":"etag-{number}"}}"#),
        );
    }
    http.post(
        &format!("https://cachet.test/{nar_key}?uploadId=up-1"),
        204,
        "",
    );
}

fn inputs(is_default_branch: bool) -> PushInputs {
    PushInputs {
        cache_url: "https://cachet.test".to_string(),
        audience: "cachet-test".to_string(),
        project: "lane-org-lane-repo".to_string(),
        installables: vec![".#leafroot".to_string()],
        is_default_branch,
    }
}

#[tokio::test]
async fn the_happy_path_uploads_and_renews_in_order() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n{ROOTED}\n"));
    commands
        .path_info
        .lock()
        .expect("map")
        .insert(".#leafroot".to_string(), Ok(format!("{ROOTED}\n")));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    seed_pair(
        &commands,
        "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        survivor_body(BUILT, &nar_key),
        &nar_key,
        vec![b'y'; 2_000],
    );
    // The root is held; BUILT is absent. One bulk answer carries both
    // verdicts.
    let http = FakeHttp::default();
    stub_probe(&http, &["rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"]);
    http.put(
        &format!("https://cachet.test/nar/{}.nar.zst", "n".repeat(52)),
        204,
        "",
    );
    http.put(
        "https://cachet.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        204,
        "",
    );
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (event_log, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("the pipeline answers");

    assert_eq!(outcome.added_paths, 2);
    assert_eq!(outcome.uploaded_objects, 2);
    assert!(outcome.lease_renewed);

    let calls = http.calls();
    let order: Vec<(&str, &str)> = calls
        .iter()
        .map(|call| (call.method.as_str(), call.url.as_str()))
        .collect();
    let put_nar = order
        .iter()
        .position(|(m, u)| *m == "PUT" && u.contains("/nar/"))
        .expect("a NAR PUT");
    let put_narinfo = order
        .iter()
        .position(|(m, u)| *m == "PUT" && u.ends_with(".narinfo"))
        .expect("a narinfo PUT");
    assert!(put_nar < put_narinfo, "NARs upload first: {order:?}");
    let renew = calls
        .iter()
        .find(|call| call.url.ends_with("/roots/lane-org-lane-repo"))
        .expect("the renewal POST");
    let renewal: serde_json::Value = serde_json::from_slice(&renew.body).expect("json body");
    assert_eq!(renewal["installables"][0], ".#leafroot");
    assert_eq!(renewal["storePaths"][0], ROOTED);

    let nar_call = &calls[put_nar];
    assert_eq!(nar_call.body.len(), 2_000);
    assert!(
        nar_call
            .bearer
            .as_deref()
            .is_some_and(|t| t.starts_with("token-"))
    );

    let events = event_log.lock().expect("events").clone();
    assert!(
        events.iter().any(|e| e.starts_with("LeaseRenewed")),
        "{events:?}"
    );
    assert_one_probe_to_one_place(&calls);
}

/// The presence question is one bulk request naming both candidates, and
/// nothing ever leaves for a foreign cache again. Factored out so the
/// happy path stays under the lint's line budget.
fn assert_one_probe_to_one_place(calls: &[Call]) {
    let probes: Vec<&Call> = calls
        .iter()
        .filter(|call| call.url == "https://cachet.test/api/probe")
        .collect();
    assert_eq!(probes.len(), 1, "exactly one bulk probe: {calls:?}");
    let probe_body: serde_json::Value =
        serde_json::from_slice(&probes[0].body).expect("the probe body parses");
    let asked: Vec<&str> = probe_body["paths"]
        .as_array()
        .expect("paths is an array")
        .iter()
        .map(|v| v.as_str().expect("a hash"))
        .collect();
    assert_eq!(
        asked,
        vec![
            "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
            "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
        ],
        "the probe names both candidate hashes"
    );
    assert!(
        calls
            .iter()
            .all(|call| call.url.starts_with("https://cachet.test/")),
        "every request goes to cachet and nowhere else: {calls:?}",
    );
}

#[tokio::test]
async fn mints_once_across_a_happy_run() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    seed_pair(
        &commands,
        NARINFO_KEY,
        survivor_body(BUILT, &nar_key),
        &nar_key,
        vec![b'y'; 2_000],
    );
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.put(&format!("https://cachet.test/{nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let inner = FakeTokens::default();
    let tokens = crate::oidc::RunTokens::over(&inner);
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("the run answers");
    assert_eq!(
        inner.mints(),
        1,
        "one mint rides the probe pass, the uploads, and the lease"
    );
}

#[tokio::test]
async fn a_401_remints_for_the_next_attempt() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    seed_pair(
        &commands,
        NARINFO_KEY,
        survivor_body(BUILT, &nar_key),
        &nar_key,
        vec![b'y'; 2_000],
    );
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.put(&format!("https://cachet.test/{nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 401, "");
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let inner = FakeTokens::default();
    let tokens = crate::oidc::RunTokens::over(&inner);
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("the retry answers");
    assert_eq!(inner.mints(), 2, "the 401 invalidated, the retry reminted");
    let calls = http.calls();
    let narinfo_put = calls
        .iter()
        .rfind(|call| call.method == "PUT" && call.url.ends_with(".narinfo"))
        .expect("the retried narinfo PUT");
    assert_eq!(narinfo_put.bearer.as_deref(), Some("token-2"));
}

#[tokio::test]
async fn nothing_added_skips_the_lease() {
    let commands = FakeCommands::with_store("/nix/store/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq-old\n");
    let http = FakeHttp::default();
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (event_log, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "/nix/store/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq-old\n",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("answers");
    assert_eq!(outcome.added_paths, 0);
    assert!(!outcome.lease_renewed);
    assert!(http.calls().is_empty(), "no wire traffic at all");
    let events = event_log.lock().expect("events").clone();
    assert!(events.iter().any(|e| e.starts_with("NothingAdded")));
}

#[tokio::test]
async fn closure_inflation_uploads_survivor_pairs_only() {
    // nix copy stages the survivor's closure; the decoys below are that
    // closure inhabited the staging tree. Their files are deliberately
    // never seeded in `files`, so an upload that any one of them owes a
    // read to dies loudly instead of sailing a wrong wire set.
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    let decoy_narinfo = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narinfo";
    let decoy_nar = format!("nar/{}.nar.zst", "m".repeat(52));
    let narinfo_body = survivor_body(BUILT, &nar_key);
    commands.layout.lock().expect("layout").extend([
        (NARINFO_KEY.to_string(), narinfo_body.len() as u64),
        (nar_key.clone(), 160_u64),
        (decoy_narinfo.to_string(), 90_u64),
        (decoy_nar, 42_000_u64),
    ]);
    commands.files.lock().expect("files").extend([
        (NARINFO_KEY.to_string(), narinfo_body.into_bytes()),
        (nar_key.clone(), vec![b'z'; 160]),
    ]);
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.put(&format!("https://cachet.test/{nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("the survivor pair uploads");

    assert_eq!(
        outcome.uploaded_objects, 2,
        "exactly the survivor's own narinfo and NAR"
    );
    let calls = http.calls();
    let puts: Vec<&str> = calls
        .iter()
        .filter(|call| call.method == "PUT")
        .map(|call| call.url.as_str())
        .collect();
    assert_eq!(
        puts,
        vec![
            format!("https://cachet.test/{nar_key}"),
            format!("https://cachet.test/{NARINFO_KEY}"),
        ],
        "NAR first, narinfo second, no closure passengers"
    );
}

#[tokio::test]
async fn a_fully_cached_rebuild_renews_without_uploading() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let http = FakeHttp::default();
    stub_probe(&http, &["qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"]);
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("answers");
    assert_eq!(outcome.uploaded_objects, 0);
    assert!(outcome.lease_renewed, "a fully-cached rebuild still renews");
    let calls = http.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.url == "https://cachet.test/api/probe")
            .count(),
        1,
        "the rerun asked its one bulk question: {calls:?}",
    );
}

#[tokio::test]
async fn the_multipart_quartet_carries_the_contract() {
    let big = cachet_core::constants::UPLOAD_PART_BYTES * 2 + 13;
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = stage_big_nar(&commands, big);
    let narinfo_body = survivor_body(BUILT, &nar_key);
    commands
        .layout
        .lock()
        .expect("layout")
        .extend([(NARINFO_KEY.to_string(), narinfo_body.len() as u64)]);
    commands
        .files
        .lock()
        .expect("files")
        .insert(NARINFO_KEY.to_string(), narinfo_body.into_bytes());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    stub_quartet_ok(&http, &nar_key, 3);
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("answers");

    let calls = http.calls();
    let begin = calls
        .iter()
        .find(|call| call.url.ends_with("?uploads"))
        .expect("the begin POST");
    assert_eq!(
        begin.headers,
        vec![("x-cachet-upload-bytes".to_string(), big.to_string())],
    );
    let complete = calls
        .iter()
        .find(|call| {
            call.method == "POST"
                && call.url.contains("uploadId=up-1")
                && !call.url.contains("uploads")
        })
        .expect("the completion POST");
    let parts: serde_json::Value = serde_json::from_slice(&complete.body).expect("json parts");
    assert_eq!(
        parts,
        serde_json::json!([
            {"partNumber": 1, "etag": "etag-1"},
            {"partNumber": 2, "etag": "etag-2"},
            {"partNumber": 3, "etag": "etag-3"},
        ]),
    );
    let final_part = calls
        .iter()
        .find(|call| call.url.contains("partNumber=3"))
        .expect("the small part");
    assert_eq!(
        final_part.body.len(),
        13,
        "the final part carries exactly the remainder"
    );
}

#[tokio::test]
async fn nars_finish_before_any_narinfo_under_fanout() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n{OTHER}\n"));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    let other_nar_key = format!("nar/{}.nar.zst", "m".repeat(52));
    seed_pair(
        &commands,
        NARINFO_KEY,
        survivor_body(BUILT, &nar_key),
        &nar_key,
        vec![b'a'; 1_000],
    );
    seed_pair(
        &commands,
        OTHER_KEY,
        survivor_body(OTHER, &other_nar_key),
        &other_nar_key,
        vec![b'b'; 1_000],
    );
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.put(&format!("https://cachet.test/{nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{other_nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 204, "");
    http.put(&format!("https://cachet.test/{OTHER_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("answers");
    assert_eq!(outcome.uploaded_objects, 4);
    let calls = http.calls();
    let puts: Vec<&Call> = calls.iter().filter(|call| call.method == "PUT").collect();
    let last_nar = puts
        .iter()
        .rposition(|call| call.url.contains("/nar/"))
        .expect("a NAR PUT");
    let first_narinfo = puts
        .iter()
        .position(|call| call.url.ends_with(".narinfo"))
        .expect("a narinfo PUT");
    assert!(
        last_nar < first_narinfo,
        "the phase barrier holds under waves: {puts:?}",
    );
}

#[tokio::test]
async fn a_failed_nar_wave_blocks_the_next_wave() {
    // Eighteen survivors (= two NAR waves): one NAR from wave one dies,
    // so wave two's NARs and every narinfo must never see the wire.
    const N: usize = 18;
    let paths: Vec<String> = (0..N)
        .map(|i| format!("/nix/store/{i:0>31}p-built-{i}"))
        .collect();
    let commands = FakeCommands::with_store(&format!("{}\n", paths.join("\n")));
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    for (i, path) in paths.iter().enumerate() {
        let narinfo_key = format!("{i:0>31}p.narinfo");
        let nar_key = format!("nar/{i:0>51}n.nar.zst");
        seed_pair(
            &commands,
            &narinfo_key,
            survivor_body(path, &nar_key),
            &nar_key,
            vec![b'x'; 100],
        );
        if i == 2 {
            for _ in 0..3 {
                http.fail_put(&format!("https://cachet.test/{nar_key}"), "boom");
            }
        } else if i < 16 {
            http.put(&format!("https://cachet.test/{nar_key}"), 204, "");
        }
    }
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let failure = push(
        &a,
        &inputs(false),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect_err("the dying NAR ends the run");
    assert!(
        failure.to_string().contains("failed after 3 attempts"),
        "{failure}",
    );
    let calls = http.calls();
    assert!(
        !calls
            .iter()
            .any(|call| call.method == "PUT" && call.url.ends_with(".narinfo")),
        "no narinfo rode out of a failed NAR wave",
    );
}

#[tokio::test]
async fn a_dying_part_aborts_best_effort() {
    let big = cachet_core::constants::UPLOAD_SINGLE_MAX_BYTES + 13;
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = stage_big_nar(&commands, big);
    let narinfo_body = survivor_body(BUILT, &nar_key);
    commands
        .layout
        .lock()
        .expect("layout")
        .extend([(NARINFO_KEY.to_string(), narinfo_body.len() as u64)]);
    commands
        .files
        .lock()
        .expect("files")
        .insert(NARINFO_KEY.to_string(), narinfo_body.into_bytes());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.post(
        &format!("https://cachet.test/{nar_key}?uploads"),
        200,
        r#"{"uploadId":"up-1","expectedParts":2}"#,
    );
    http.fail_put(
        &format!("https://cachet.test/{nar_key}?uploadId=up-1&partNumber=1"),
        "connection reset",
    );
    http.fail_put(
        &format!("https://cachet.test/{nar_key}?uploadId=up-1&partNumber=1"),
        "connection reset",
    );
    http.fail_put(
        &format!("https://cachet.test/{nar_key}?uploadId=up-1&partNumber=1"),
        "connection reset",
    );
    // Part 2 rides the same wave as the dying part: it answers, and the
    // wave still fails with the plan-order-first error.
    http.put(
        &format!("https://cachet.test/{nar_key}?uploadId=up-1&partNumber=2"),
        200,
        r#"{"partNumber":2,"etag":"etag-2"}"#,
    );
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let failure = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect_err("the part dies");
    let text = failure.to_string();
    assert!(
        text.contains("part 1 of") && text.contains("failed after 3 attempts"),
        "{text}",
    );
    assert_eq!(
        http.delete_log.lock().expect("log").len(),
        1,
        "the abort fired once",
    );
}

#[tokio::test]
async fn off_the_default_branch_the_lease_sleeps() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let http = FakeHttp::default();
    stub_probe(&http, &["qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"]);
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(false),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("answers");
    assert!(!outcome.lease_renewed);
    assert!(
        !http.calls().iter().any(|call| call.url.contains("/roots/")),
        "no renewal attempted",
    );
}

const OTHER: &str = "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-built-2";
const OTHER_KEY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.narinfo";

#[tokio::test]
async fn the_probe_answers_the_upload_set() {
    // BUILT is held, OTHER is absent: one bulk answer decides both fates.
    let commands = FakeCommands::with_store(&format!("{BUILT}\n{OTHER}\n"));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    let other_nar_key = format!("nar/{}.nar.zst", "m".repeat(52));
    seed_pair(
        &commands,
        NARINFO_KEY,
        survivor_body(BUILT, &nar_key),
        &nar_key,
        vec![b'a'; 1_000],
    );
    seed_pair(
        &commands,
        OTHER_KEY,
        survivor_body(OTHER, &other_nar_key),
        &other_nar_key,
        vec![b'b'; 1_000],
    );
    let http = FakeHttp::default();
    stub_probe(&http, &["qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"]);
    http.put(&format!("https://cachet.test/{other_nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{OTHER_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_events, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("answers");

    assert_eq!(outcome.cache_hits, 1);
    assert_eq!(outcome.uploaded_objects, 2, "exactly OTHER's pair");
    let calls = http.calls();
    let puts: Vec<&str> = calls
        .iter()
        .filter(|call| call.method == "PUT")
        .map(|call| call.url.as_str())
        .collect();
    assert_eq!(
        puts,
        vec![
            format!("https://cachet.test/{other_nar_key}"),
            format!("https://cachet.test/{OTHER_KEY}"),
        ],
        "the held path's pair never ships: {puts:?}",
    );
}

#[tokio::test]
async fn a_failed_probe_treats_everything_as_absent() {
    // The probe dies all three attempts: the run pushes the world anyway,
    // because re-uploading re-signs identical bytes while a false "held"
    // would strand clients on a 404. The event says so.
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = format!("nar/{}.nar.zst", "n".repeat(52));
    seed_pair(
        &commands,
        NARINFO_KEY,
        survivor_body(BUILT, &nar_key),
        &nar_key,
        vec![b'y'; 2_000],
    );
    let http = FakeHttp::default();
    for _ in 0..3 {
        http.fail_post("https://cachet.test/api/probe", "connection reset");
    }
    http.put(&format!("https://cachet.test/{nar_key}"), 204, "");
    http.put(&format!("https://cachet.test/{NARINFO_KEY}"), 204, "");
    http.post("https://cachet.test/roots/lane-org-lane-repo", 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let a = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (event_log, mut sink) = collect_events();
    let outcome = push(
        &a,
        &inputs(true),
        "",
        std::path::Path::new("/staging"),
        &mut sink,
    )
    .await
    .expect("the fallback pushes");

    assert_eq!(outcome.cache_hits, 0);
    assert_eq!(outcome.uploaded_objects, 2);
    let events = event_log.lock().expect("events").clone();
    assert!(
        events.iter().any(|e| e.starts_with("ProbeBulkFailed")),
        "the run names its fallback: {events:?}",
    );
}
