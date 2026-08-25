//! The doctor probe: answers "is this machine wired to read" the way an
//! operator does, but mechanically. The load-bearing distinction is
//! 404-versus-401: the worker authenticates before it consults the
//! bucket, so a 404 proves the credential works (the probed hash is
//! simply absent) and a 401 proves it does not.

/// One check's outcome. `ok` answers the summary; `detail` explains the
/// failure to the human reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// What was checked, one short clause.
    pub name: String,
    /// Whether it held.
    pub ok: bool,
    /// What the check answer meant, or what to do about it.
    pub detail: String,
}

impl Probe {
    fn pass(name: &str, detail: String) -> Self {
        Self {
            name: name.to_string(),
            ok: true,
            detail,
        }
    }

    fn fail(name: &str, detail: String) -> Self {
        Self {
            name: name.to_string(),
            ok: false,
            detail,
        }
    }
}

/// A hash that parses but cannot exist: all zero bits but the last.
const PROBE_NARINFO: &str = "0000000000000000000000000000000a.narinfo";

/// Run every probe against one deployment: the unauthenticated health
/// document, the public config, the refusal of an anonymous read, and —
/// when a token is stored — the authenticated read, GitHub identity,
/// and org membership. Absence of a stored token is itself a probe
/// outcome, not an error.
pub async fn run_doctor(
    client: &reqwest::Client,
    cache_url: &str,
    token: Option<&str>,
    api_base: &str,
) -> Vec<Probe> {
    let mut probes = vec![probe_cache_info(client, cache_url).await];
    let (config_probe, config) = probe_config(client, cache_url).await;
    probes.push(config_probe);
    probes.push(probe_narinfo(client, cache_url, None, true).await);
    let Some(token) = token else {
        probes.push(Probe::fail(
            "a stored login exists",
            format!("no token for this host: run `cachet login --cache-url {cache_url}`"),
        ));
        return probes;
    };
    probes.push(Probe::pass(
        "a stored login exists",
        "the state directory holds a token".to_string(),
    ));
    probes.push(probe_narinfo(client, cache_url, Some(token), false).await);
    probes.push(probe_identity(client, api_base, token).await);
    if let Some(config) = config {
        probes.push(probe_memberships(client, api_base, token, &config.orgs).await);
    }
    probes
}

/// The health document: unauthenticated by design, and its body names
/// the store directory.
async fn probe_cache_info(client: &reqwest::Client, cache_url: &str) -> Probe {
    const NAME: &str = "the cache answers /nix-cache-info";
    match client
        .get(format!("{cache_url}/nix-cache-info"))
        .send()
        .await
    {
        Ok(answer) if answer.status().is_success() => {
            let body = answer.text().await.unwrap_or_default();
            if body.contains("StoreDir: /nix/store") {
                Probe::pass(NAME, format!("{cache_url} is serving"))
            } else {
                Probe::fail(
                    NAME,
                    "the body names no store dir: is this a cachet deployment?".to_string(),
                )
            }
        }
        Ok(answer) => Probe::fail(NAME, format!("answered {}", answer.status().as_u16())),
        Err(failure) => Probe::fail(NAME, format!("no answer: {failure}")),
    }
}

/// The config document: OAuth client id, orgs, host, and the public
/// key. The probe keeps the document for the membership checks.
async fn probe_config(
    client: &reqwest::Client,
    cache_url: &str,
) -> (Probe, Option<cachet_api::PublicConfig>) {
    match crate::login::fetch_public_config(client, cache_url).await {
        Ok(config) => {
            let probe = Probe::pass(
                "the public config serves",
                format!(
                    "host {}, orgs [{}], key {}",
                    config.host,
                    config.orgs.join(", "),
                    config.public_key.split(':').next().unwrap_or("?")
                ),
            );
            (probe, Some(config))
        }
        Err(failure) => (
            Probe::fail("the public config serves", failure.to_string()),
            None,
        ),
    }
}

/// The load-bearing probe, run twice: anonymous must refuse with 401
/// (auth before bucket work); the stored token must clear auth, which
/// the impossible hash then renders as a 404.
async fn probe_narinfo(
    client: &reqwest::Client,
    cache_url: &str,
    token: Option<&str>,
    anonymous: bool,
) -> Probe {
    let (name, refused_detail) = if anonymous {
        (
            "anonymous reads refuse",
            "401 before the bucket, as the auth order requires",
        )
    } else {
        (
            "the stored token reads",
            "404 means authenticated-and-absent: the wiring works",
        )
    };
    let mut request = client.get(format!("{cache_url}/{PROBE_NARINFO}"));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let expect = if anonymous { 401 } else { 404 };
    match request.send().await {
        Ok(answer) if answer.status().as_u16() == expect => {
            Probe::pass(name, refused_detail.to_string())
        }
        Ok(answer) if !anonymous && answer.status().is_success() => Probe::pass(
            name,
            "the probe path unexpectedly exists, which still proves auth".to_string(),
        ),
        Ok(answer) => Probe::fail(
            name,
            if anonymous {
                format!(
                    "answered {} instead of 401: the read guard is misconfigured",
                    answer.status().as_u16()
                )
            } else {
                format!(
                    "answered {}: this machine's credential expired or was revoked; run `cachet login` again, then `cachet setup`",
                    answer.status().as_u16()
                )
            },
        ),
        Err(failure) => Probe::fail(name, format!("no answer: {failure}")),
    }
}

/// Who the token says the human is: a sanity check that the flow's
/// answer still works, and the handle the summary prints.
async fn probe_identity(client: &reqwest::Client, api_base: &str, token: &str) -> Probe {
    const NAME: &str = "the token identifies";
    match client
        .get(format!("{api_base}/user"))
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(answer) if answer.status().is_success() => {
            #[derive(serde::Deserialize)]
            struct User {
                login: Option<String>,
            }
            match answer.json::<User>().await {
                Ok(User { login: Some(login) }) => {
                    Probe::pass(NAME, format!("authenticates as {login}"))
                }
                _ => Probe::fail(NAME, "GitHub answered without a login".to_string()),
            }
        }
        Ok(answer) => Probe::fail(
            NAME,
            format!(
                "GitHub answered {}: the credential is revoked or expired; run `cachet login` again",
                answer.status().as_u16()
            ),
        ),
        Err(failure) => Probe::fail(NAME, format!("no answer: {failure}")),
    }
}

/// The worker's admission test, checked from the outside: membership in
/// at least one served org.
async fn probe_memberships(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    orgs: &[String],
) -> Probe {
    const NAME: &str = "org membership holds";
    let mut memberships = 0_usize;
    for org in orgs {
        let url = format!("{api_base}/user/memberships/orgs/{org}");
        if matches!(
            client.get(&url).bearer_auth(token).send().await,
            Ok(answer) if answer.status().is_success()
        ) {
            memberships += 1;
        }
    }
    if memberships > 0 {
        Probe::pass(
            NAME,
            format!("a member of {memberships} of {} served org(s)", orgs.len()),
        )
    } else {
        Probe::fail(
            NAME,
            format!(
                "a member of none of [{}]: the worker will refuse every read",
                orgs.join(", ")
            ),
        )
    }
}

/// Render the probe list in the report's shape: an `ok`/`FAIL` line per
/// probe. Answers whether everything held.
#[must_use]
pub fn render(probes: &[Probe]) -> (Vec<String>, bool) {
    let mut all_ok = true;
    let mut lines = Vec::new();
    for probe in probes {
        if probe.ok {
            lines.push(format!("ok   {}: {}", probe.name, probe.detail));
        } else {
            all_ok = false;
            lines.push(format!("FAIL {}: {}", probe.name, probe.detail));
        }
    }
    (lines, all_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_separates_ok_from_fail() {
        let probes = vec![
            Probe::pass("a", "fine".to_string()),
            Probe::fail("b", "broken".to_string()),
        ];
        let (lines, all_ok) = render(&probes);
        assert!(!all_ok);
        assert_eq!(lines[0], "ok   a: fine");
        assert_eq!(lines[1], "FAIL b: broken");
        let (happy, all_ok) = render(&[Probe::pass("a", "fine".to_string())]);
        assert!(all_ok);
        assert_eq!(happy.len(), 1);
    }
}
