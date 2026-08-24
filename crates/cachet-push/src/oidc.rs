//! The OIDC mint: GitHub answers one request with one JsonWebToken. One
//! run rides one mint through `RunTokens`, and the retry envelope's 401
//! hook is what keeps the old "fresh credential on every retry"
//! guarantee exactly where it is needed.

use crate::adapters::{Http, TokenSource};
use crate::error::PushError;

/// The two environment variables the mint answers to. Their absence is a
/// configuration fact with its own sentence, because the fix lives in the
/// job's YAML, not here.
#[derive(Debug)]
pub struct OidcEnv {
    /// `ACTIONS_ID_TOKEN_REQUEST_URL`, verbatim.
    pub request_url: String,
    /// `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, verbatim.
    pub request_token: String,
}

/// Read the mint's environment from explicit pairs, so tests never touch
/// ambient state.
///
/// # Errors
///
/// [`PushError::OidcUnavailable`] naming the YAML fix.
pub fn oidc_env(vars: &[(String, String)]) -> Result<OidcEnv, PushError> {
    let find = |name: &str| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.clone())
    };
    let url = find("ACTIONS_ID_TOKEN_REQUEST_URL");
    let token = find("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    match (url, token) {
        (Some(request_url), Some(request_token))
            if !request_url.is_empty() && !request_token.is_empty() =>
        {
            Ok(OidcEnv {
                request_url,
                request_token,
            })
        }
        _ => Err(PushError::OidcUnavailable(
            "no OIDC token request variables. Add `permissions: { id-token: write }` to the job."
                .to_string(),
        )),
    }
}

/// RFC 3986 unreserved pass-through for the audience parameter.
fn encode_component(value: &str) -> String {
    use std::fmt::Write as _;
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if UNRESERVED.contains(byte) {
            out.push(char::from(*byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The mint over a generic wire. GitHub's request URL already carries its
/// query; the audience appends as another `&` parameter, verbatim with
/// the previous implementation.
pub struct OidcTokens<'a, H> {
    env: &'a OidcEnv,
    http: &'a H,
}

impl<'a, H: Http> OidcTokens<'a, H> {
    /// Borrow pieces.
    pub fn new(env: &'a OidcEnv, http: &'a H) -> Self {
        Self { env, http }
    }
}

#[derive(Debug, serde::Deserialize)]
struct MintAnswer {
    value: Option<String>,
}

impl<H: Http> TokenSource for OidcTokens<'_, H> {
    async fn mint(&self, audience: &str) -> Result<String, PushError> {
        let url = format!(
            "{}&audience={}",
            self.env.request_url,
            encode_component(audience),
        );
        let answer = self
            .http
            .get(&url, Some(&self.env.request_token))
            .await
            .map_err(|failure| {
                PushError::OidcUnavailable(format!("the OIDC token request failed: {failure}"))
            })?;
        if !(200..300).contains(&answer.status) {
            return Err(PushError::OidcUnavailable(format!(
                "the OIDC token request returned {}",
                answer.status,
            )));
        }
        let parsed: MintAnswer = serde_json::from_slice(&answer.body).map_err(|failure| {
            PushError::OidcUnavailable(format!("the OIDC token answer did not parse: {failure}"))
        })?;
        parsed
            .value
            .filter(|token| !token.is_empty())
            .ok_or(PushError::OidcEmpty)
    }
}

/// A run-scoped mint: the first call reaches the real source, later
/// calls reuse the run's token, and a 401 anywhere clears the slot so
/// the next attempt remints fresh. why: freshness is detected by the
/// authoritative refusal, never by a client clock, so no `exp` decode
/// and no skew class enter the design. The guard is a std Mutex whose
/// borrow never crosses an await.
pub struct RunTokens<'a, T: TokenSource> {
    inner: &'a T,
    slot: std::sync::Mutex<Option<String>>,
}

impl<'a, T: TokenSource> RunTokens<'a, T> {
    /// Wrap a source for one run's worth of sharing.
    pub fn over(inner: &'a T) -> Self {
        Self {
            inner,
            slot: std::sync::Mutex::new(None),
        }
    }
}

impl<T: TokenSource> TokenSource for RunTokens<'_, T> {
    async fn mint(&self, audience: &str) -> Result<String, PushError> {
        if let Some(token) = self.slot.lock().expect("the memo").clone() {
            return Ok(token);
        }
        let token = self.inner.mint(audience).await?;
        *self.slot.lock().expect("the memo") = Some(token.clone());
        Ok(token)
    }

    async fn invalidate(&self, _audience: &str) {
        *self.slot.lock().expect("the memo") = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_variables_name_the_yaml_fix() {
        match oidc_env(&[]) {
            Err(PushError::OidcUnavailable(message)) => {
                assert!(message.contains("id-token: write"), "{message}");
            }
            other => panic!("expected the config error, got {other:?}"),
        }
        assert!(
            oidc_env(&[(
                "ACTIONS_ID_TOKEN_REQUEST_URL".to_string(),
                "https://x".to_string(),
            )])
            .is_err()
        );
    }

    #[test]
    fn the_audience_encodes_minimally() {
        assert_eq!(encode_component("cachet"), "cachet");
        assert_eq!(
            encode_component("api:GitHub Actions"),
            "api%3AGitHub%20Actions"
        );
    }
}
