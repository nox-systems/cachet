//! GitHub device-flow login, the laptop's path to a read credential. The
//! deployment's public config names the OAuth App; GitHub answers the
//! flow directly, the worker is never in the loop, and the token lands
//! under the CLI's state directory at 0600.

use crate::CliError;

/// github.com's web half: device code and token polls.
pub const GITHUB_WEB_BASE: &str = "https://github.com";
/// The API half: identity checks.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// The scope laptop reads need: org membership is the worker's whole
/// admission test.
const DEVICE_SCOPE: &str = "read:org";

/// One sleep, injected so tests never wait. Milliseconds.
pub type Sleeper =
    dyn Fn(u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync;

/// What a poll of the token endpoint answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollAnswer {
    /// The user has not entered the code yet.
    Pending,
    /// The server asked for a longer interval.
    SlowDown,
    /// The flow completed.
    Granted {
        /// The OAuth user token.
        token: String,
        /// The refresh token, when the OAuth App issues expiring
        /// tokens. Empty otherwise, which is an app whose tokens never
        /// expire and so never need renewing.
        refresh_token: String,
        /// Seconds the access token lasts, zero when it does not
        /// expire.
        expires_in_seconds: u64,
    },
    /// The device code lapsed before the user finished.
    Expired,
    /// The user declined.
    Denied {
        /// GitHub's own reason text.
        description: String,
    },
}

/// The session the device-code endpoint hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSession {
    /// The code polls are keyed by.
    pub device_code: String,
    /// The code the human types.
    pub user_code: String,
    /// The page the human opens.
    pub verification_uri: String,
    /// How long the session lives, seconds.
    pub expires_in_secs: u64,
    /// The starting poll interval, seconds.
    pub interval_secs: u64,
}

/// The two endpoints a flow needs, plus the identity check that makes
/// the success line honest. Scripted in tests, reqwest in production.
pub trait DeviceServer: Send + Sync {
    /// Ask for a device session.
    fn start(
        &self,
        client_id: &str,
        scope: &str,
    ) -> impl std::future::Future<Output = Result<DeviceSession, CliError>> + Send;

    /// One poll of the token endpoint.
    fn poll(
        &self,
        client_id: &str,
        device_code: &str,
    ) -> impl std::future::Future<Output = Result<PollAnswer, CliError>> + Send;

    /// The login the token authenticates as.
    fn whoami(
        &self,
        token: &str,
    ) -> impl std::future::Future<Output = Result<String, CliError>> + Send;
}

/// Fetch the deployment's public config: the OAuth App's client id, the
/// orgs, the host name, and the key laptops learn to trust.
///
/// # Errors
///
/// [`CliError`] when the URL answers badly or the body is not the
/// document.
pub async fn fetch_public_config(
    client: &reqwest::Client,
    cache_url: &str,
) -> Result<cachet_api::PublicConfig, CliError> {
    let url = format!("{cache_url}/api/public/config");
    let answer = client
        .get(&url)
        .send()
        .await
        .map_err(|failure| CliError(format!("{url} did not answer: {failure}")))?;
    if !answer.status().is_success() {
        return Err(CliError(format!(
            "{url} answered {}: this cache is not serving its public config",
            answer.status().as_u16()
        )));
    }
    answer
        .json::<cachet_api::PublicConfig>()
        .await
        .map_err(|failure| CliError(format!("{url} did not return a public config: {failure}")))
}
/// What GitHub hands back when the flow completes: the access token,
/// and, where the OAuth App issues expiring tokens, what is needed to
/// renew it without asking the person again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GithubGrant {
    /// The access token.
    pub token: String,
    /// The refresh token, empty when the app's tokens do not expire.
    pub refresh_token: String,
    /// Seconds the access token lasts, zero when it does not expire.
    pub expires_in_seconds: u64,
}

/// Drive one flow: emit the instructions, poll until an outcome, and
/// identify the human at the end. Answers the token and the login it
/// authenticates as.
///
/// # Errors
///
/// [`CliError`] on denial, expiry, transport failures, or a server that
/// stops making sense.
pub async fn run_device_flow<D: DeviceServer>(
    server: &D,
    client_id: &str,
    sleep_ms: &Sleeper,
    tell: &mut dyn FnMut(&str),
) -> Result<(GithubGrant, String), CliError> {
    let session = server.start(client_id, DEVICE_SCOPE).await?;
    tell(&format!(
        "cachet: open {} and enter {}",
        session.verification_uri, session.user_code
    ));
    let mut interval_ms = session.interval_secs.max(1) * 1000;
    let mut waited_ms = 0_u64;
    let budget_ms = session.expires_in_secs.max(1) * 1000;
    let grant = loop {
        sleep_ms(interval_ms).await;
        waited_ms += interval_ms;
        if waited_ms > budget_ms {
            return Err(CliError(
                "the device code expired before the flow finished; run `cachet login` again"
                    .to_string(),
            ));
        }
        match server.poll(client_id, &session.device_code).await? {
            PollAnswer::Pending => {}
            PollAnswer::SlowDown => interval_ms += 5000,
            PollAnswer::Granted {
                token,
                refresh_token,
                expires_in_seconds,
            } => {
                break GithubGrant {
                    token,
                    refresh_token,
                    expires_in_seconds,
                };
            }
            PollAnswer::Expired => {
                return Err(CliError(
                    "the device code expired; run `cachet login` again".to_string(),
                ));
            }
            PollAnswer::Denied { description } => {
                return Err(CliError(format!(
                    "the flow was declined{}",
                    if description.is_empty() {
                        String::new()
                    } else {
                        format!(": {description}")
                    }
                )));
            }
        }
    };
    let login = server.whoami(&grant.token).await?;
    Ok((grant, login))
}

/// The reqwest-backed server, pointed at the real GitHub hosts.
pub struct GithubLive<'a> {
    client: &'a reqwest::Client,
    web_base: &'a str,
    api_base: &'a str,
}

impl<'a> GithubLive<'a> {
    /// Borrow the client against the default GitHub hosts.
    #[must_use]
    pub fn new(client: &'a reqwest::Client) -> Self {
        Self {
            client,
            web_base: GITHUB_WEB_BASE,
            api_base: GITHUB_API_BASE,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct DeviceCodeAnswer {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    // Present only when the OAuth App issues expiring tokens; GitHub
    // omits both for an app whose tokens never expire.
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct UserAnswer {
    login: Option<String>,
}

fn describe(error: Option<String>, description: Option<String>) -> String {
    match (error, description) {
        (Some(code), Some(text)) => format!("{code}: {text}"),
        (Some(code), None) => code,
        (None, Some(text)) => text,
        (None, None) => "an answer with no fields".to_string(),
    }
}

impl DeviceServer for GithubLive<'_> {
    async fn start(&self, client_id: &str, scope: &str) -> Result<DeviceSession, CliError> {
        let url = format!("{}/login/device/code", self.web_base);
        let answer = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .form(&[("client_id", client_id), ("scope", scope)])
            .send()
            .await
            .map_err(|failure| CliError(format!("{url} did not answer: {failure}")))?;
        let parsed: DeviceCodeAnswer = answer.json().await.map_err(|failure| {
            CliError(format!("{url} did not return a device session: {failure}"))
        })?;
        match parsed {
            DeviceCodeAnswer {
                device_code: Some(device_code),
                user_code: Some(user_code),
                verification_uri: Some(verification_uri),
                expires_in,
                interval,
                ..
            } => Ok(DeviceSession {
                device_code,
                user_code,
                verification_uri,
                expires_in_secs: expires_in.unwrap_or(900),
                interval_secs: interval.unwrap_or(5),
            }),
            other => Err(CliError(format!(
                "{url} refused the device flow: {}",
                describe(other.error, other.error_description)
            ))),
        }
    }

    async fn poll(&self, client_id: &str, device_code: &str) -> Result<PollAnswer, CliError> {
        let url = format!("{}/login/oauth/access_token", self.web_base);
        let answer = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .form(&[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|failure| CliError(format!("{url} did not answer: {failure}")))?;
        let parsed: TokenAnswer = answer.json().await.map_err(|failure| {
            CliError(format!("{url} did not return a token answer: {failure}"))
        })?;
        if let Some(token) = parsed.access_token.filter(|t| !t.is_empty()) {
            return Ok(PollAnswer::Granted {
                token,
                refresh_token: parsed.refresh_token.unwrap_or_default(),
                expires_in_seconds: parsed.expires_in.unwrap_or(0),
            });
        }
        Ok(match parsed.error.as_deref().unwrap_or("") {
            "authorization_pending" => PollAnswer::Pending,
            "slow_down" => PollAnswer::SlowDown,
            "expired_token" => PollAnswer::Expired,
            other => PollAnswer::Denied {
                description: if other.is_empty() {
                    describe(None, parsed.error_description)
                } else {
                    describe(Some(other.to_string()), parsed.error_description)
                },
            },
        })
    }

    async fn whoami(&self, token: &str) -> Result<String, CliError> {
        let url = format!("{}/user", self.api_base);
        let answer = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|failure| CliError(format!("{url} did not answer: {failure}")))?;
        if !answer.status().is_success() {
            return Err(CliError(format!(
                "the new token could not answer {url} ({}): the flow granted nothing usable",
                answer.status().as_u16()
            )));
        }
        let parsed: UserAnswer = answer
            .json()
            .await
            .map_err(|failure| CliError(format!("{url} did not return a user: {failure}")))?;
        parsed
            .login
            .filter(|l| !l.is_empty())
            .ok_or_else(|| CliError(format!("{url} answered without a login field")))
    }
}

/// Trade a proven GitHub identity for the deployment's own read
/// credential.
///
/// The GitHub token goes over the wire once, here. What comes back is
/// what this machine keeps and what the nix daemon carries, so GitHub's
/// eight-hour token lifetime never reaches the daemon's netrc and the
/// deployment is never left holding a credential replayable against
/// github.com (ADR 0002).
///
/// # Errors
///
/// [`CliError`] when the deployment refuses the identity or cannot be
/// reached, with the deployment's own reason where it gave one.
pub async fn exchange_for_read_token(
    client: &reqwest::Client,
    cache_url: &str,
    grant: &GithubGrant,
) -> Result<cachet_api::ReadTokenIssued, CliError> {
    let url = format!("{}/api/login/exchange", cache_url.trim_end_matches('/'));
    // The refresh token goes with it, so the deployment can renew the
    // GitHub credential itself and nobody has to log in again when
    // GitHub's eight hours are up (ADR 0002).
    let response = client
        .post(&url)
        .bearer_auth(&grant.token)
        .json(&cachet_api::LoginExchangeBody {
            refresh_token: grant.refresh_token.clone(),
            expires_in_seconds: grant.expires_in_seconds,
        })
        .send()
        .await
        .map_err(|failure| CliError(format!("could not reach {url}: {failure}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(CliError(format!(
            "{cache_url} serves an organization this account is not a member of"
        )));
    }
    if !status.is_success() {
        return Err(CliError(format!(
            "{cache_url} refused the login ({status}): {}",
            body.trim()
        )));
    }
    serde_json::from_str(&body).map_err(|failure| {
        CliError(format!(
            "{cache_url} answered an unreadable login: {failure}"
        ))
    })
}

/// Ask the deployment to forget one credential.
///
/// # Errors
///
/// [`CliError`] when the deployment cannot be reached or refuses.
pub async fn revoke_read_token(
    client: &reqwest::Client,
    cache_url: &str,
    token: &str,
) -> Result<(), CliError> {
    let url = format!("{}/api/login/revoke", cache_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(token)
        .header("content-length", "0")
        .send()
        .await
        .map_err(|failure| CliError(format!("could not reach {url}: {failure}")))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(CliError(format!(
            "{cache_url} answered {}",
            response.status()
        )))
    }
}

/// An expiry as a person reads it: the date, in UTC, no clock time.
///
/// The exact second a credential dies is nobody's business; what a
/// reader wants is whether it outlasts the trip they are packing for.
#[must_use]
pub fn render_expiry(expires_at_ms: u64) -> String {
    let seconds = i64::try_from(expires_at_ms / 1_000).unwrap_or(0);
    chrono::DateTime::from_timestamp(seconds, 0).map_or_else(
        || "at a time this build cannot read".to_string(),
        |when| when.format("on %Y-%m-%d (UTC)").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted server: sessions on demand, poll answers in order.
    struct FakeServer {
        session: DeviceSession,
        polls: Mutex<std::collections::VecDeque<Result<PollAnswer, CliError>>>,
        login: String,
    }

    impl DeviceServer for FakeServer {
        async fn start(&self, client_id: &str, scope: &str) -> Result<DeviceSession, CliError> {
            assert_eq!(client_id, "the-client-id");
            assert_eq!(scope, DEVICE_SCOPE);
            Ok(self.session.clone())
        }
        async fn poll(&self, _client_id: &str, _device_code: &str) -> Result<PollAnswer, CliError> {
            self.polls
                .lock()
                .expect("polls")
                .pop_front()
                .expect("a scripted poll")
        }
        async fn whoami(&self, _token: &str) -> Result<String, CliError> {
            Ok(self.login.clone())
        }
    }

    fn fake_server(polls: Vec<PollAnswer>) -> FakeServer {
        FakeServer {
            session: DeviceSession {
                device_code: "dev".to_string(),
                user_code: "CAFE-1234".to_string(),
                verification_uri: "https://github.com/login/device".to_string(),
                expires_in_secs: 900,
                interval_secs: 1,
            },
            polls: Mutex::new(polls.into_iter().map(Ok).collect()),
            login: "a-user".to_string(),
        }
    }

    fn no_sleep() -> Box<Sleeper> {
        Box::new(|_ms| Box::pin(async {}))
    }

    #[tokio::test]
    async fn pending_then_granted_prints_the_instructions() {
        let server = fake_server(vec![
            PollAnswer::Pending,
            PollAnswer::Granted {
                token: "gho_tok".to_string(),
                refresh_token: "ghr_tok".to_string(),
                expires_in_seconds: 28_800,
            },
        ]);
        let sleep = no_sleep();
        let mut lines = Vec::new();
        let (grant, login) = run_device_flow(&server, "the-client-id", &sleep, &mut |line| {
            lines.push(line.to_string());
        })
        .await
        .expect("flow");
        assert_eq!(grant.token, "gho_tok");
        // The renewal half rides along, so the deployment can keep the
        // GitHub credential fresh without another login.
        assert_eq!(grant.refresh_token, "ghr_tok");
        assert_eq!(grant.expires_in_seconds, 28_800);
        assert_eq!(login, "a-user");
        assert_eq!(
            lines,
            vec!["cachet: open https://github.com/login/device and enter CAFE-1234".to_string()]
        );
    }

    #[tokio::test]
    async fn slow_down_lengthens_and_denial_names_the_reason() {
        let server = fake_server(vec![
            PollAnswer::SlowDown,
            PollAnswer::Granted {
                token: "gho_tok".to_string(),
                refresh_token: "ghr_tok".to_string(),
                expires_in_seconds: 28_800,
            },
        ]);
        let sleep = no_sleep();
        let (grant, _) = run_device_flow(&server, "the-client-id", &sleep, &mut |_| {})
            .await
            .expect("flow");
        let token = grant.token;
        assert_eq!(token, "gho_tok");

        let server = fake_server(vec![PollAnswer::Denied {
            description: "access_denied".to_string(),
        }]);
        let sleep = no_sleep();
        let failure = run_device_flow(&server, "the-client-id", &sleep, &mut |_| {})
            .await
            .expect_err("denied");
        assert!(failure.0.contains("access_denied"), "{failure}");
    }

    #[tokio::test]
    async fn the_budget_closes_on_expiry() {
        let server = FakeServer {
            session: DeviceSession {
                device_code: "dev".to_string(),
                user_code: "CAFE-1234".to_string(),
                verification_uri: "u".to_string(),
                expires_in_secs: 2,
                interval_secs: 1,
            },
            polls: Mutex::new(
                [
                    PollAnswer::Pending,
                    PollAnswer::Pending,
                    PollAnswer::Pending,
                ]
                .into_iter()
                .map(Ok)
                .collect(),
            ),
            login: "a-user".to_string(),
        };
        let sleep = no_sleep();
        let failure = run_device_flow(&server, "the-client-id", &sleep, &mut |_| {})
            .await
            .expect_err("expired");
        assert!(failure.0.contains("expired"), "{failure}");
    }
}
