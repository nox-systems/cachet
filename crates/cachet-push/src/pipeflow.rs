//! The scripted end-to-end evidence: fake adapters answer, the pipeline
//! drives, and the request log answers what the wire saw. These tests are
//! the unit lane's analog of the workerd lane for the write path's other
//! half.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use crate::adapters::{Adapters, Commands, Http, TokenSource, WireAnswer};
use crate::error::PushError;
use crate::pipeline::{PushEvent, PushInputs, Sleeper, push};

/// A scripted nix+fs adapter.
#[derive(Default)]
struct FakeCommands {
    path_info_all: Mutex<VecDeque<Result<String, PushError>>>,
    path_info: Mutex<BTreeMap<String, Result<String, PushError>>>,
    copy_log: Mutex<Vec<(String, Vec<String>)>>,
    layout: Mutex<Vec<(String, u64)>>,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
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
    async fn read_dir(&self, _dir: &std::path::Path) -> Result<Vec<(String, u64)>, PushError> {
        Ok(self.layout.lock().expect("layout").clone())
    }
    async fn read_file(&self, path: &std::path::Path) -> Result<Vec<u8>, PushError> {
        self.files
            .lock()
            .expect("files")
            .get(
                path.file_name()
                    .expect("a name")
                    .to_str()
                    .expect("utf8 names"),
            )
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
        let bytes = files
            .get(
                path.file_name()
                    .expect("a name")
                    .to_str()
                    .expect("utf8 names"),
            )
            .ok_or(PushError::Detail {
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
    fn head(&self, url: &str, status: u16) {
        self.head_answers
            .lock()
            .expect("answers")
            .entry(url.to_string())
            .or_default()
            .push_back(status);
    }
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

/// Both presence probes miss for the fixture narinfo.
fn stub_probes_miss(http: &FakeHttp) {
    http.head(&format!("https://upstream.test/{NARINFO_KEY}"), 404);
    http.head(&format!("https://cachet.test/{NARINFO_KEY}"), 404);
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
        format!("{}.nar.zst", "n".repeat(52)),
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
        upstream_url: "https://upstream.test".to_string(),
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
    commands.layout.lock().expect("layout").extend([
        (
            "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo".to_string(),
            400_u64,
        ),
        (format!("nar/{}.nar.zst", "n".repeat(52)), 2_000_u64),
    ]);
    commands.files.lock().expect("files").extend([
        (
            "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo".to_string(),
            vec![b'x'; 400],
        ),
        (format!("{}.nar.zst", "n".repeat(52)), vec![b'y'; 2_000]),
    ]);
    // Root kept unprobed; BUILT absent upstream AND absent at cachet.
    let http = FakeHttp::default();
    http.head(
        "https://upstream.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        404,
    );
    http.head(
        "https://cachet.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        404,
    );
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
async fn a_fully_cached_rebuild_renews_without_uploading() {
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let http = FakeHttp::default();
    http.head(
        "https://upstream.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        404,
    );
    http.head(
        "https://cachet.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        200,
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
}

#[tokio::test]
async fn the_multipart_quartet_carries_the_contract() {
    let big = cachet_core::constants::UPLOAD_PART_BYTES * 2 + 13;
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    commands
        .layout
        .lock()
        .expect("layout")
        .extend([(NARINFO_KEY.to_string(), 400_u64)]);
    commands
        .files
        .lock()
        .expect("files")
        .insert(NARINFO_KEY.to_string(), vec![b'x'; 400]);
    let nar_key = stage_big_nar(&commands, big);
    let http = FakeHttp::default();
    stub_probes_miss(&http);
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
async fn a_dying_part_aborts_best_effort() {
    let big = cachet_core::constants::UPLOAD_SINGLE_MAX_BYTES + 13;
    let commands = FakeCommands::with_store(&format!("{BUILT}\n"));
    let nar_key = stage_big_nar(&commands, big);
    let http = FakeHttp::default();
    stub_probes_miss(&http);
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
    http.head(
        "https://upstream.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        404,
    );
    http.head(
        "https://cachet.test/qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq.narinfo",
        200,
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
