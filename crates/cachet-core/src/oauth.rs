//! The browser OAuth flow's pure decisions: what the login redirect
//! carries, what the callback must present, and the cookie strings the
//! wire sees. Entropy stays outside (CLAUDE.md §3): identifiers arrive as
//! sixteen bytes the worker sampled at the edge, and this module only
//! formats them.

use base64ct::{Base64UrlUnpadded, Encoding};

use crate::auth::oauth_state_valid;
use crate::constants::{OAUTH_STATE_TTL_MS, SESSION_COOKIE_NAME, SESSION_TTL_MS};
use crate::error::{ClientError, Result};
use crate::types::UnixMillis;

/// The scope the login asks for. `read:user` answers who, `read:org`
/// answers the membership the verdict needs; nothing more would read
/// anything cachet uses.
pub const OAUTH_SCOPE: &str = "read:org read:user";

/// The path the callback route serves, as one place both routes agree on.
pub const CALLBACK_PATH: &str = "/_auth/callback";

/// The identifier alphabet: 128 bits is unguessable by construction, and
/// base64url keeps the value cookie-safe and query-safe without padding.
pub fn id_from_random(random: &[u8; 16]) -> String {
    Base64UrlUnpadded::encode_string(random)
}

/// The GitHub authorize URL the login route redirects to.
pub fn authorize_url(web_base: &str, client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{web_base}/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        encode_query(client_id),
        encode_query(redirect_uri),
        encode_query(OAUTH_SCOPE),
        encode_query(state),
    )
}

/// The token-exchange form the callback posts. The client secret passes
/// through here because secrecy lives in the binding, not in the code
/// path: this string never leaves the worker.
pub fn exchange_form(client_id: &str, client_secret: &str, code: &str) -> String {
    format!(
        "client_id={}&client_secret={}&code={}",
        encode_query(client_id),
        encode_query(client_secret),
        encode_query(code),
    )
}

/// The callback's two query parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackParams {
    /// The exchange code GitHub issued.
    pub code: String,
    /// The state the login minted, echoed back.
    pub state: String,
}

/// Parse the callback query. Both fields are mandatory and nonempty: a
/// partial callback is a malformed request, and GitHub's `error` query is
/// deliberately not read, since a refused authorization lands as a missing
/// code all the same.
///
/// # Errors
///
/// [`ClientError::MalformedOauth`] when `code` or `state` is absent,
/// empty, or undecodable.
pub fn parse_callback_query(query: &str) -> Result<CallbackParams> {
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "code" => code = Some(decode_query(value)?),
            "state" => state = Some(decode_query(value)?),
            _ => {}
        }
    }
    match (code, state) {
        (Some(code), Some(state)) if !code.is_empty() && !state.is_empty() => {
            Ok(CallbackParams { code, state })
        }
        _ => Err(ClientError::MalformedOauth),
    }
}

/// The KV record under `oauth-state/{state}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OAuthStateRecord {
    /// When the state was minted, in epoch milliseconds.
    #[serde(rename = "issuedAtMs")]
    pub issued_at_ms: u64,
}

/// Whether a stored state is still inside its window. Delegates to the
/// auth policy so the login flow and the verdict flow age documents the
/// same way.
pub fn state_live(record: &OAuthStateRecord, now: UnixMillis) -> bool {
    oauth_state_valid(record.issued_at_ms, now)
}

/// The Set-Cookie a successful callback answers with. Max-Age matches the
/// KV TTL so the cookie and the session expire together instead of
/// stranding one half.
pub fn session_cookie(session_id: &str) -> String {
    format!(
        "{SESSION_COOKIE_NAME}={session_id}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        SESSION_TTL_MS / 1_000,
    )
}

/// The logout Set-Cookie: the same name and path with an immediate
/// expiry, which is what makes a browser actually drop the cookie.
pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

/// Where a successful callback lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackTarget {
    /// 302 to the deployment's own console.
    Redirect(String),
    /// 204 with the body empty, for a deployment that cannot name its
    /// own host and so cannot address its own console.
    Empty,
}

/// The console's path on the deployment's own host.
pub const CONSOLE_PATH: &str = "/console";

/// Where a person who just signed in lands: the console they came from.
///
/// There is no configuration here, and there was until the console
/// existed. A `CACHET_UI_ORIGIN` pointing somewhere else redirected a
/// browser to a UI that could not authenticate: the session cookie is
/// SameSite=Lax on this host, the worker emits no CORS headers and
/// answers OPTIONS with a 404, so the destination had no credential and
/// no way to acquire one. Turning a deployment's console off is a
/// different thing than redirecting past it, and this was never that
/// either: the assets upload unconditionally, so `/console` served
/// regardless and the setting only stranded the person signing in.
#[must_use]
pub fn callback_target(host: &str) -> CallbackTarget {
    if host.is_empty() {
        return CallbackTarget::Empty;
    }
    CallbackTarget::Redirect(format!("https://{host}{CONSOLE_PATH}"))
}

/// The form body that trades a refresh token for a fresh access token.
///
/// No client secret: GitHub waives it when the token being refreshed
/// came from the device flow, which is every credential this path holds.
#[must_use]
pub fn refresh_form(client_id: &str, refresh_token: &str) -> String {
    format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}",
        encode_query(client_id),
        encode_query(refresh_token),
    )
}

/// RFC 3986 unreserved pass-through for query values. GitHub matches
/// redirect_uri by exact string, so the encoding must be minimal,
/// deterministic, and total: every input encodes, nothing encodes twice.
pub fn encode_query(value: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if UNRESERVED.contains(byte) {
            out.push(char::from(*byte));
        } else {
            out.push('%');
            out.push(char::from(b"0123456789ABCDEF"[usize::from(byte >> 4)]));
            out.push(char::from(b"0123456789ABCDEF"[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// Percent-decoding for the callback's parameters. `+` stays a literal
/// plus: our own state never contains one, and guessing at form semantics
/// for GitHub's code buys nothing.
fn decode_query(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .ok_or(ClientError::MalformedOauth)?;
            let decoded = u8::from_str_radix(
                std::str::from_utf8(hex).map_err(|_| ClientError::MalformedOauth)?,
                16,
            )
            .map_err(|_| ClientError::MalformedOauth)?;
            out.push(decoded);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ClientError::MalformedOauth)
}

/// The TTL the login gives a state in KV, in KV's seconds.
pub const fn state_ttl_seconds() -> u64 {
    OAUTH_STATE_TTL_MS / 1_000
}

/// The TTL the callback gives a session in KV, in KV's seconds.
pub const fn session_ttl_seconds() -> u64 {
    SESSION_TTL_MS / 1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identifier_is_url_safe_base64() {
        let random = [0xff_u8; 16];
        let id = id_from_random(&random);
        assert_eq!(id.len(), 22);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert!(!id.contains('='));
    }

    #[test]
    fn the_authorize_url_carries_the_full_contract() {
        let url = authorize_url(
            "https://github.com",
            "client-1",
            "https://cachet.example.com/_auth/callback",
            "state-1",
        );
        assert_eq!(
            url,
            "https://github.com/login/oauth/authorize?client_id=client-1&redirect_uri=https%3A%2F%2Fcachet.example.com%2F_auth%2Fcallback&scope=read%3Aorg%20read%3Auser&state=state-1",
        );
    }

    #[test]
    fn the_exchange_form_encodes_every_value() {
        assert_eq!(
            exchange_form("client-1", "secret/with=chars", "code 1"),
            "client_id=client-1&client_secret=secret%2Fwith%3Dchars&code=code%201",
        );
    }

    #[test]
    fn the_callback_query_parses_and_refuses() {
        let parsed =
            parse_callback_query("code=abc&state=xyz").expect("a complete callback parses");
        assert_eq!(
            parsed,
            CallbackParams {
                code: "abc".to_string(),
                state: "xyz".to_string(),
            },
        );
        assert_eq!(
            parse_callback_query("code=a%2Fb&state=x%2Dy")
                .expect("percent escapes decode")
                .code,
            "a/b",
        );
        for bad in [
            "",
            "code=abc",
            "state=xyz",
            "code=&state=x",
            "code=%zz&state=x",
            "error=access_denied",
        ] {
            assert_eq!(
                parse_callback_query(bad),
                Err(ClientError::MalformedOauth),
                "{bad} refused",
            );
        }
    }

    #[test]
    fn the_state_record_ages_on_the_shared_policy() {
        let record = OAuthStateRecord {
            issued_at_ms: 1_000_000,
        };
        assert!(state_live(&record, UnixMillis::new(1_000_000)));
        assert!(!state_live(
            &record,
            UnixMillis::new(1_000_000 + OAUTH_STATE_TTL_MS),
        ));
    }

    #[test]
    fn the_cookie_strings_are_exact() {
        assert_eq!(
            session_cookie("abc"),
            "cachet_session=abc; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=1209600",
        );
        assert_eq!(
            clear_session_cookie(),
            "cachet_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0",
        );
    }

    #[test]
    fn the_callback_lands_on_the_deployment_s_own_console() {
        assert_eq!(
            callback_target("cachet.example.com"),
            CallbackTarget::Redirect("https://cachet.example.com/console".to_string())
        );
        // A deployment that cannot name its own host cannot address its
        // own console, and says nothing rather than redirecting nowhere.
        assert_eq!(callback_target(""), CallbackTarget::Empty);
    }
}
