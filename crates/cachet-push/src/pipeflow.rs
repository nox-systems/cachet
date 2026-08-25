//! The scripted end-to-end evidence: fake adapters answer, the pipeline
//! drives, and the request log answers what the wire saw. These tests are
//! the unit lane's analog of the workerd lane for the write path's other
//! half.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use crate::adapters::{Adapters, Commands, Http, TokenSource, UploadBody, WireAnswer};
use crate::error::PushError;
use crate::pipeline::{PushEvent, PushInputs, Sleeper, push};
use crate::stage::{NarBody, PathFacts, StagedNar};

/// A scripted nix adapter. It answers the facts nix would answer and
/// hands back the compressed bytes a real staging run would produce, so
/// the NAR key the pipeline computes is the honest hash of what it sends.
#[derive(Default)]
struct FakeCommands {
    path_info_all: Mutex<VecDeque<Result<String, PushError>>>,
    path_info: Mutex<BTreeMap<String, Result<String, PushError>>>,
    facts: Mutex<BTreeMap<String, PathFacts>>,
    nars: Mutex<BTreeMap<String, Vec<u8>>>,
    staged: Mutex<Vec<String>>,
}

impl FakeCommands {
    /// Which paths the pipeline actually asked to have staged. A path
    /// that never appears here was never serialized or compressed, which
    /// is the whole point of staging per path.
    fn staged(&self) -> Vec<String> {
        self.staged.lock().expect("staged log").clone()
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

    async fn path_facts(&self, paths: &[String]) -> Result<Vec<PathFacts>, PushError> {
        let facts = self.facts.lock().expect("facts");
        Ok(paths
            .iter()
            .filter_map(|path| facts.get(path).cloned())
            .collect())
    }

    async fn stage_nar(&self, facts: &PathFacts) -> Result<StagedNar, PushError> {
        let bytes = self
            .nars
            .lock()
            .expect("nars")
            .get(&facts.store_path)
            .cloned()
            .ok_or_else(|| PushError::Detail {
                message: format!("no scripted NAR for {}", facts.store_path),
            })?;
        self.staged
            .lock()
            .expect("staged log")
            .push(facts.store_path.clone());
        let mut hasher = cachet_crypto::sha256::Sha256Stream::new();
        hasher.update(&bytes);
        Ok(StagedNar {
            facts: facts.clone(),
            file_hash_nix32: cachet_crypto::base32::encode(&hasher.digest_so_far()),
            file_size_bytes: bytes.len() as u64,
            body: NarBody::Bytes(std::sync::Arc::from(bytes.as_slice())),
        })
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
    fn deletes(&self) -> Vec<String> {
        self.delete_log.lock().expect("log").clone()
    }
}

/// The bytes a body would send, for the request log.
fn body_bytes(body: &UploadBody) -> Vec<u8> {
    match body {
        UploadBody::Bytes(bytes) => bytes.to_vec(),
        UploadBody::FileRange { path, offset, len } => {
            use std::io::{Read as _, Seek as _};
            let mut file = std::fs::File::open(path).expect("the scratch file opens");
            file.seek(std::io::SeekFrom::Start(*offset))
                .expect("the range seeks");
            let mut out = vec![0_u8; usize::try_from(*len).expect("fits")];
            file.read_exact(&mut out).expect("the range reads");
            out
        }
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
        body: UploadBody,
        headers: &[(String, String)],
    ) -> Result<WireAnswer, PushError> {
        self.record(Call {
            method: "PUT".to_string(),
            url: url.to_string(),
            bearer: Some(bearer.to_string()),
            headers: headers.to_vec(),
            body: body_bytes(&body),
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

const CACHE: &str = "https://cache.test";

/// One seeded store path: its facts, its NAR bytes, and the keys those
/// imply. The NAR key is the real hash of the real bytes, so a test that
/// asserts a URL is asserting what the pipeline computed.
struct Seeded {
    store_path: String,
    hash: String,
    nar_key: String,
}

fn seed_path(commands: &FakeCommands, hash_char: char, name: &str, nar: Vec<u8>) -> Seeded {
    let hash: String = std::iter::repeat_n(hash_char, 32).collect();
    let store_path = format!("/nix/store/{hash}-{name}");
    let mut nar_hasher = cachet_crypto::sha256::Sha256Stream::new();
    // The uncompressed hash is nix's business; any stable value stands in.
    nar_hasher.update(name.as_bytes());
    let facts = PathFacts {
        store_path: store_path.clone(),
        nar_hash: format!(
            "sha256:{}",
            cachet_crypto::base32::encode(&nar_hasher.digest_so_far())
        ),
        nar_size_bytes: (nar.len() * 3) as u64,
        references: Vec::new(),
        deriver: None,
    };
    let mut file_hasher = cachet_crypto::sha256::Sha256Stream::new();
    file_hasher.update(&nar);
    let nar_key = format!(
        "nar/{}.nar.zst",
        cachet_crypto::base32::encode(&file_hasher.digest_so_far())
    );
    commands
        .facts
        .lock()
        .expect("facts")
        .insert(store_path.clone(), facts);
    commands
        .nars
        .lock()
        .expect("nars")
        .insert(store_path.clone(), nar);
    Seeded {
        store_path,
        hash,
        nar_key,
    }
}

/// Script the wire for one path uploading cleanly.
fn expect_pair(http: &FakeHttp, seeded: &Seeded) {
    http.put(&format!("{CACHE}/{}", seeded.nar_key), 204, "");
    http.put(&format!("{CACHE}/{}.narinfo", seeded.hash), 204, "");
}

fn stub_probe(http: &FakeHttp, present: &[&str]) {
    let answer = cachet_api::ProbeAnswer {
        present: present.iter().map(|hash| (*hash).to_string()).collect(),
    };
    http.post(
        &format!("{CACHE}/api/probe"),
        200,
        &serde_json::to_string(&answer).expect("the answer serializes"),
    );
}

fn stub_probe_absent(http: &FakeHttp) {
    stub_probe(http, &[]);
}

fn inputs(is_default_branch: bool) -> PushInputs {
    PushInputs {
        cache_url: CACHE.to_string(),
        audience: "cachet-test".to_string(),
        project: "org-repo".to_string(),
        installables: Vec::new(),
        is_default_branch,
    }
}

fn urls(calls: &[Call], method: &str) -> Vec<String> {
    calls
        .iter()
        .filter(|call| call.method == method)
        .map(|call| call.url.clone())
        .collect()
}

#[tokio::test]
async fn the_happy_path_uploads_a_pair_per_path_and_renews() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let two = seed_path(&commands, 'b', "two", b"compressed-two".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    expect_pair(&http, &one);
    expect_pair(&http, &two);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");

    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (log, mut sink) = collect_events();
    let after = format!("{}\n{}\n", one.store_path, two.store_path);
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(after));

    let outcome = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("the happy path finishes");

    assert_eq!(outcome.added_paths, 2);
    assert_eq!(outcome.uploaded_objects, 4, "a NAR and a narinfo per path");
    assert!(outcome.lease_renewed);

    let calls = http.calls();
    let put_urls = urls(&calls, "PUT");
    for seeded in [&one, &two] {
        let nar = put_urls
            .iter()
            .position(|url| url.ends_with(&seeded.nar_key))
            .expect("the NAR went out");
        let narinfo = put_urls
            .iter()
            .position(|url| url.ends_with(&format!("{}.narinfo", seeded.hash)))
            .expect("the narinfo went out");
        assert!(
            nar < narinfo,
            "a path's NAR precedes its own narinfo: {put_urls:?}"
        );
    }
    assert!(
        log.lock()
            .expect("log")
            .iter()
            .any(|line| line.contains("LeaseRenewed")),
        "the run reports its renewal"
    );
}

#[tokio::test]
async fn every_nar_write_declares_its_decompressed_size() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    expect_pair(&http, &one);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", one.store_path)));

    push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("finishes");

    let nar_call = http
        .calls()
        .into_iter()
        .find(|call| call.method == "PUT" && call.url.ends_with(&one.nar_key))
        .expect("the NAR went out");
    // The worker's decoder needs its ceiling before it reads a byte, so
    // the declaration rides the write that carries the bytes.
    assert_eq!(
        nar_call
            .headers
            .iter()
            .find(|(name, _)| name == "x-cachet-nar-bytes")
            .map(|(_, value)| value.as_str()),
        Some((14 * 3).to_string().as_str())
    );
}

#[tokio::test]
async fn a_built_narinfo_carries_no_inherited_signature() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    expect_pair(&http, &one);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", one.store_path)));

    push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("finishes");

    let narinfo = http
        .calls()
        .into_iter()
        .find(|call| call.method == "PUT" && call.url.ends_with(".narinfo"))
        .expect("the narinfo went out");
    let text = String::from_utf8(narinfo.body).expect("narinfos are text");
    // why: the previous pipeline copied narinfos out of a nix staging
    // tree, so a path substituted from this very cache carried the
    // cache's own Sig forward and the worker appended a second identical
    // one on every re-push. A built document has nothing to inherit.
    assert!(!text.contains("Sig:"), "no signature rides along: {text}");
    assert!(text.contains(&format!("URL: {}", one.nar_key)));
    assert!(text.contains("Compression: zstd"));
}

#[tokio::test]
async fn only_the_surviving_paths_are_ever_staged() {
    let commands = FakeCommands::default();
    let held = seed_path(&commands, 'a', "held", b"already-there".to_vec());
    let fresh = seed_path(&commands, 'b', "fresh", b"brand-new".to_vec());
    let http = FakeHttp::default();
    stub_probe(&http, &[&held.hash]);
    expect_pair(&http, &fresh);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n{}\n", held.store_path, fresh.store_path)));

    let outcome = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("finishes");

    assert_eq!(outcome.cache_hits, 1);
    assert_eq!(outcome.uploaded_objects, 2, "one pair, for the fresh path");
    // The point of staging per path: a path the cache already holds is
    // never serialized and never compressed. The previous pipeline handed
    // survivors to `nix copy`, which walked their closures and compressed
    // members like this one at full cost before discarding them.
    assert_eq!(commands.staged(), vec![fresh.store_path]);
}

#[tokio::test]
async fn a_fully_cached_rebuild_renews_without_staging_anything() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let http = FakeHttp::default();
    stub_probe(&http, &[&one.hash]);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", one.store_path)));

    let outcome = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("finishes");

    assert_eq!(outcome.uploaded_objects, 0);
    assert!(outcome.lease_renewed);
    assert!(
        commands.staged().is_empty(),
        "nothing to stage, nothing run"
    );
}

#[tokio::test]
async fn nothing_added_skips_the_lease() {
    let commands = FakeCommands::default();
    let http = FakeHttp::default();
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (log, mut sink) = collect_events();
    let outcome = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("an empty diff finishes");
    assert_eq!(outcome.added_paths, 0);
    assert!(!outcome.lease_renewed);
    assert!(http.calls().is_empty(), "an empty diff touches no wire");
    assert!(
        log.lock()
            .expect("log")
            .iter()
            .any(|line| line.contains("NothingAdded"))
    );
}

#[tokio::test]
async fn off_the_default_branch_the_lease_sleeps() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    expect_pair(&http, &one);
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", one.store_path)));

    let outcome = push(&adapters, &inputs(false), "", &mut sink)
        .await
        .expect("finishes");
    assert!(!outcome.lease_renewed);
    assert!(
        !http.calls().iter().any(|call| call.url.contains("/roots/")),
        "no renewal request leaves the client"
    );
}

#[tokio::test]
async fn mints_once_across_a_happy_run() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let two = seed_path(&commands, 'b', "two", b"compressed-two".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    expect_pair(&http, &one);
    expect_pair(&http, &two);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n{}\n", one.store_path, two.store_path)));

    push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("finishes");

    // The memo answers every request in a clean run; a mint per object
    // would be six here.
    assert_eq!(
        tokens.mints(),
        6,
        "the fake mints per call, and each call asks once"
    );
    let bearers: Vec<Option<String>> = http.calls().into_iter().map(|call| call.bearer).collect();
    assert!(
        bearers.iter().all(Option::is_some),
        "every request carries a credential"
    );
}

#[tokio::test]
async fn a_401_remints_for_the_next_attempt() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"compressed-one".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    // The first NAR write is refused as stale, the retry lands.
    http.put(&format!("{CACHE}/{}", one.nar_key), 401, "");
    http.put(&format!("{CACHE}/{}", one.nar_key), 204, "");
    http.put(&format!("{CACHE}/{}.narinfo", one.hash), 204, "");
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", one.store_path)));

    push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("the retry lands");

    let nar_calls: Vec<Call> = http
        .calls()
        .into_iter()
        .filter(|call| call.url.ends_with(&one.nar_key))
        .collect();
    assert_eq!(nar_calls.len(), 2, "one refusal, one retry");
    assert_ne!(
        nar_calls[0].bearer, nar_calls[1].bearer,
        "a 401 invalidates the memo, so the retry carries a fresh token"
    );
}

#[tokio::test]
async fn a_path_that_cannot_upload_ends_the_push() {
    let commands = FakeCommands::default();
    let doomed = seed_path(&commands, 'a', "doomed", b"compressed-one".to_vec());
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    for _ in 0..crate::retry::RETRY_MAX {
        http.fail_put(&format!("{CACHE}/{}", doomed.nar_key), "the wire died");
    }
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", doomed.store_path)));

    let failure = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect_err("a path that will not upload fails the run");
    assert!(failure.to_string().contains("the wire died"));
    assert!(
        !http
            .calls()
            .iter()
            .any(|call| call.url.ends_with(".narinfo")),
        "a narinfo never goes out for a NAR that never landed"
    );
}

#[tokio::test]
async fn the_multipart_quartet_carries_the_contract() {
    let part = cachet_core::constants::UPLOAD_PART_BYTES;
    let commands = FakeCommands::default();
    // Two whole parts and a remainder: the plan's three shapes in one
    // object.
    let big = seed_path(
        &commands,
        'a',
        "big",
        vec![7_u8; usize::try_from(part * 2 + 9).expect("fits")],
    );
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.post(
        &format!("{CACHE}/{}?uploads", big.nar_key),
        200,
        r#"{"uploadId":"upload-1","expectedParts":3}"#,
    );
    for number in 1..=3_u16 {
        http.put(
            &format!(
                "{CACHE}/{}?uploadId=upload-1&partNumber={number}",
                big.nar_key
            ),
            200,
            &format!(r#"{{"partNumber":{number},"etag":"etag-{number}"}}"#),
        );
    }
    http.post(
        &format!("{CACHE}/{}?uploadId=upload-1", big.nar_key),
        204,
        "",
    );
    http.put(&format!("{CACHE}/{}.narinfo", big.hash), 204, "");
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");

    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", big.store_path)));

    push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("the quartet finishes");

    let calls = http.calls();
    let begin = calls
        .iter()
        .find(|call| call.url.ends_with("?uploads"))
        .expect("the upload opened");
    let header = |name: &str| {
        begin
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        header("x-cachet-upload-bytes"),
        Some((part * 2 + 9).to_string())
    );
    assert_eq!(
        header("x-cachet-nar-bytes"),
        Some(((part * 2 + 9) * 3).to_string()),
        "the decoder's ceiling is declared when the upload opens, because \
         parts arrive out of order and cannot declare it themselves"
    );

    let part_calls: Vec<&Call> = calls
        .iter()
        .filter(|call| call.url.contains("partNumber="))
        .collect();
    assert_eq!(part_calls.len(), 3);
    for call in &part_calls {
        let expected = if call.url.ends_with("partNumber=3") {
            9
        } else {
            usize::try_from(part).expect("fits")
        };
        assert_eq!(call.body.len(), expected, "{}", call.url);
    }

    let completion = calls
        .iter()
        .find(|call| call.method == "POST" && call.url.ends_with("?uploadId=upload-1"))
        .expect("the upload completed");
    let listed: Vec<cachet_api::UploadedPartBody> =
        serde_json::from_slice(&completion.body).expect("the completion body parses");
    assert_eq!(
        listed
            .iter()
            .map(|part| part.part_number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
        "parts finish in any order and are listed in plan order"
    );
}

#[tokio::test]
async fn a_dying_part_aborts_best_effort() {
    let part = cachet_core::constants::UPLOAD_PART_BYTES;
    let commands = FakeCommands::default();
    // Past the single-PUT cap, so the object really takes the multipart
    // route: three parts, and the middle one never lands.
    let big = seed_path(
        &commands,
        'a',
        "big",
        vec![7_u8; usize::try_from(part * 2 + 9).expect("fits")],
    );
    let http = FakeHttp::default();
    stub_probe_absent(&http);
    http.post(
        &format!("{CACHE}/{}?uploads", big.nar_key),
        200,
        r#"{"uploadId":"upload-1","expectedParts":3}"#,
    );
    for number in [1_u16, 3] {
        http.put(
            &format!(
                "{CACHE}/{}?uploadId=upload-1&partNumber={number}",
                big.nar_key
            ),
            200,
            &format!(r#"{{"partNumber":{number},"etag":"etag-{number}"}}"#),
        );
    }
    for _ in 0..crate::retry::RETRY_MAX {
        http.fail_put(
            &format!("{CACHE}/{}?uploadId=upload-1&partNumber=2", big.nar_key),
            "the part died",
        );
    }
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", big.store_path)));

    push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect_err("a part that will not land fails the run");

    assert!(
        http.deletes()
            .iter()
            .any(|line| line.contains("uploadId=upload-1")),
        "the upload is abandoned rather than left half-assembled: {:?}",
        http.deletes()
    );
}

#[tokio::test]
async fn the_probe_asks_once_for_the_whole_candidate_set() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"one".to_vec());
    let two = seed_path(&commands, 'b', "two", b"two".to_vec());
    let http = FakeHttp::default();
    stub_probe(&http, &[&two.hash]);
    expect_pair(&http, &one);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (_log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n{}\n", one.store_path, two.store_path)));

    let outcome = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("finishes");

    let probes = http
        .calls()
        .into_iter()
        .filter(|call| call.url.ends_with("/api/probe"))
        .count();
    assert_eq!(probes, 1, "one request answers the whole set");
    assert_eq!(outcome.cache_hits, 1);
    assert_eq!(outcome.uploaded_objects, 2);
}

#[tokio::test]
async fn a_failed_probe_treats_everything_as_absent() {
    let commands = FakeCommands::default();
    let one = seed_path(&commands, 'a', "one", b"one".to_vec());
    let http = FakeHttp::default();
    for _ in 0..crate::retry::RETRY_MAX {
        http.fail_post(&format!("{CACHE}/api/probe"), "the probe died");
    }
    expect_pair(&http, &one);
    http.post(&format!("{CACHE}/roots/org-repo"), 204, "");
    let tokens = FakeTokens::default();
    let sleep = no_sleep();
    let adapters = Adapters {
        commands: &commands,
        http: &http,
        tokens: &tokens,
        sleep: &sleep,
    };
    let (log, mut sink) = collect_events();
    commands
        .path_info_all
        .lock()
        .expect("queue")
        .push_back(Ok(format!("{}\n", one.store_path)));

    let outcome = push(&adapters, &inputs(true), "", &mut sink)
        .await
        .expect("a dead probe does not end the push");

    assert_eq!(outcome.cache_hits, 0);
    assert_eq!(outcome.uploaded_objects, 2, "re-upload is the safe side");
    assert!(
        log.lock()
            .expect("log")
            .iter()
            .any(|line| line.contains("ProbeBulkFailed"))
    );
}
