//! Claude usage read from the endpoint every Claude client polls.
//!
//! The status-line capture in [`super::claude`] only produces data while an
//! interactive terminal session is open. A status line belongs to Claude Code's
//! terminal renderer; the desktop app draws its own interface in Electron, so it
//! never renders one and never runs the command. Anyone working there saw an
//! empty gauge, with nothing in their setup to correct.
//!
//! This is the source underneath every client — Claude Code's own
//! `fetchUtilization` reads the same endpoint — so it answers whether the user
//! is in a terminal, in the desktop app, or has nothing running at all.
//!
//! Two deliberate restraints:
//!
//! * Credentials are read, never written, and never refreshed. Claude Code owns
//!   those tokens and rotates them on refresh; a refresh raced from here could
//!   invalidate the copy the CLI is holding and sign the user out of their own
//!   editor. An expired token is reported as expired and left for Claude Code
//!   to repair on its next run.
//! * The endpoint is internal to Claude Code rather than a documented API, so
//!   every field is treated as optional. A shape we do not recognise has to
//!   degrade to "no reading" — never to a confidently wrong number.

use std::{fs, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{paths, settings::atomic_write_json};

use super::{claude::CapturedWindow, timestamp, ProviderFailure};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CACHE_FILE: &str = "claude-usage.json";

/// The shortest gap between two calls to the endpoint.
///
/// Deliberately decoupled from the widget's refresh interval, which the user
/// can set as low as a minute and which governs how often the *display* is
/// brought up to date — not how often a provider is allowed to make a network
/// request. Without this floor, turning the refresh rate up would silently turn
/// the request rate up with it.
///
/// Fifteen minutes is chosen against what the reading is for: a five-hour
/// window moves about a third of a percent per minute at a sustained pace, so a
/// quarter-hour-old figure is still the right number to make a decision on. The
/// cost of the floor is bounded staleness; the benefit is a hard ceiling of
/// four requests an hour no matter how the widget is configured.
const POLL_INTERVAL: i64 = 900;

/// Matches the timeout Claude Code uses for the same request, doubled for the
/// slower networks a desktop widget will sit on. This runs on the provider's
/// own thread, so it delays one refresh and nothing else.
const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Utilization {
    pub(super) five_hour: Option<CapturedWindow>,
    pub(super) seven_day: Option<CapturedWindow>,
}

/// A reading and the moment it was taken, which is not necessarily now.
pub(super) struct Reading {
    pub(super) five_hour: Option<CapturedWindow>,
    pub(super) seven_day: Option<CapturedWindow>,
    pub(super) observed_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedUsage {
    schema_version: u32,
    observed_at: i64,
    five_hour: Option<CapturedWindow>,
    seven_day: Option<CapturedWindow>,
}

/// The endpoint's view of usage, polled at most once per [`POLL_INTERVAL`].
///
/// Every refresh in between is answered from the last reading, stamped with
/// when that reading was actually taken rather than when it was served. The
/// widget refreshing more often than this costs nothing and asks nothing of
/// Anthropic.
pub(super) fn read(now: i64) -> Result<Reading, ProviderFailure> {
    if let Some(cached) = load_cache().filter(|cached| is_current(cached, now)) {
        return Ok(Reading {
            five_hour: cached.five_hour,
            seven_day: cached.seven_day,
            observed_at: cached.observed_at,
        });
    }

    let utilization = fetch(now)?;
    let reading = Reading {
        five_hour: utilization.five_hour,
        seven_day: utilization.seven_day,
        observed_at: now,
    };
    store(&reading);
    Ok(reading)
}

fn is_current(cached: &CachedUsage, now: i64) -> bool {
    cached.schema_version == 1 && now.saturating_sub(cached.observed_at) < POLL_INTERVAL
}

fn load_cache() -> Option<CachedUsage> {
    let bytes = fs::read(paths::cache_dir().join(CACHE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best effort. A cache that cannot be written costs an extra request next
/// refresh, which is not worth failing a good reading over.
fn store(reading: &Reading) {
    let cached = CachedUsage {
        schema_version: 1,
        observed_at: reading.observed_at,
        five_hour: reading.five_hour.clone(),
        seven_day: reading.seven_day.clone(),
    };
    if let Err(error) = atomic_write_json(&paths::cache_dir().join(CACHE_FILE), &cached) {
        eprintln!("Agent Gauge could not cache the Claude usage reading: {error}");
    }
}

fn fetch(now: i64) -> Result<Utilization, ProviderFailure> {
    let token = access_token(now)?;
    let body = get(&token)?;
    let utilization = parse(&body);

    if utilization.five_hour.is_none() && utilization.seven_day.is_none() {
        // A 200 with no window we recognise. The documented reason is an
        // account plan limits do not apply to (API key, Bedrock, Vertex);
        // the undocumented one is the endpoint having moved on without us.
        return Err(ProviderFailure::disconnected(
            "usage_unavailable",
            "Claude reported no rate-limit windows for this account",
        ));
    }
    Ok(utilization)
}

/// Claude Code's stored access token, if it is present and still valid.
///
/// An expired token is refused rather than sent. The request would only earn a
/// 401, and the repair is not ours to make: refreshing is Claude Code's job.
fn access_token(now: i64) -> Result<String, ProviderFailure> {
    let path = paths::claude_credentials_path();
    let bytes = fs::read(&path).map_err(|_| {
        ProviderFailure::disconnected(
            "oauth_missing",
            "Sign in to Claude Code to read usage without a terminal session",
        )
    })?;
    let credentials: Value = serde_json::from_slice(&bytes).map_err(|_| {
        ProviderFailure::disconnected("oauth_missing", "Claude sign-in details are unreadable")
    })?;
    let oauth = credentials.get("claudeAiOauth").ok_or_else(|| {
        ProviderFailure::disconnected(
            "oauth_missing",
            "Sign in to Claude Code to read usage without a terminal session",
        )
    })?;

    // Milliseconds, unlike every other timestamp either source reports.
    if let Some(expires_at) = oauth.get("expiresAt").and_then(Value::as_i64) {
        if expires_at / 1_000 <= now {
            return Err(ProviderFailure::disconnected(
                "oauth_expired",
                "Claude sign-in has expired; open Claude Code once to refresh it",
            ));
        }
    }

    oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(Into::into)
        .ok_or_else(|| {
            ProviderFailure::disconnected(
                "oauth_missing",
                "Sign in to Claude Code to read usage without a terminal session",
            )
        })
}

fn get(token: &str) -> Result<Value, ProviderFailure> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // Statuses are read below so that an auth failure can be told apart
            // from the endpoint being down; the two need different advice.
            .http_status_as_error(false)
            .build(),
    );

    let mut response = agent
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .call()
        .map_err(|error| {
            ProviderFailure::disconnected(
                "usage_unreachable",
                format!("Could not reach Claude for usage: {error}"),
            )
        })?;

    let status = response.status().as_u16();
    match status {
        200 => response.body_mut().read_json().map_err(|_| {
            ProviderFailure::new("usage_malformed", "Claude returned unreadable usage data")
        }),
        401 | 403 => Err(ProviderFailure::disconnected(
            "usage_unauthorized",
            "Claude declined the stored sign-in; open Claude Code once to refresh it",
        )),
        429 => Err(ProviderFailure::new(
            "usage_throttled",
            "Claude is rate-limiting usage checks; the next refresh will retry",
        )),
        _ => Err(ProviderFailure::new(
            "usage_http",
            format!("Claude returned HTTP {status} for usage"),
        )),
    }
}

/// The two windows Agent Gauge shows, out of the many the endpoint returns.
///
/// The response also carries per-model buckets, spend and promotional windows.
/// Reading only what is displayed keeps this from breaking when that list
/// changes, which it does.
fn parse(body: &Value) -> Utilization {
    Utilization {
        five_hour: window(body.get("five_hour")),
        seven_day: window(body.get("seven_day")),
    }
}

/// Note `utilization`, where the status line calls the same figure
/// `used_percentage`. Same number, different spelling, different source.
fn window(value: Option<&Value>) -> Option<CapturedWindow> {
    let value = value?;
    let used_percent = value.get("utilization")?.as_f64()?;
    used_percent.is_finite().then(|| CapturedWindow {
        used_percent,
        resets_at: value.get("resets_at").and_then(timestamp),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response() -> Value {
        // Trimmed from a real 200, keeping the shape and one window that is
        // null — which is how the endpoint reports a window that does not
        // apply, rather than by omitting the key.
        serde_json::json!({
            "five_hour": {
                "utilization": 11.0,
                "resets_at": "2026-08-30T21:10:00.296680+00:00",
                "locked_reason": null
            },
            "seven_day": { "utilization": 4.0, "resets_at": null },
            "seven_day_opus": null,
            "limits": [{ "kind": "session", "percent": 11 }],
            "spend": { "percent": 0 }
        })
    }

    fn cached(observed_at: i64) -> CachedUsage {
        CachedUsage {
            schema_version: 1,
            observed_at,
            five_hour: Some(CapturedWindow {
                used_percent: 11.0,
                resets_at: Some(9_000),
            }),
            seven_day: None,
        }
    }

    #[test]
    fn a_reading_inside_the_poll_interval_is_reused_rather_than_refetched() {
        // The widget refreshes every five minutes by default and can be set to
        // every minute. Neither may turn into a request.
        assert!(is_current(&cached(1_000), 1_000 + POLL_INTERVAL - 1));
    }

    #[test]
    fn the_endpoint_is_asked_again_once_the_interval_has_passed() {
        assert!(!is_current(&cached(1_000), 1_000 + POLL_INTERVAL));
    }

    #[test]
    fn the_request_ceiling_holds_at_four_an_hour() {
        // Stated in the README and worth failing a build over if it drifts.
        assert_eq!(3_600 / POLL_INTERVAL, 4);
    }

    #[test]
    fn a_cache_from_an_unknown_schema_is_refetched_rather_than_trusted() {
        let mut future = cached(1_000);
        future.schema_version = 99;

        assert!(!is_current(&future, 1_000));
    }

    /// Hits the real endpoint with the signed-in user's own token.
    ///
    /// Ignored by default so that CI and `cargo test` stay offline and
    /// deterministic. Run it by hand — `cargo test -- --ignored fetch` — after
    /// touching anything in this module: it is the only check that the
    /// undocumented shape this file parses is still the shape being served.
    #[test]
    #[ignore = "requires a signed-in Claude Code and network access"]
    fn fetch_reads_live_usage() {
        let utilization = fetch(crate::providers::now_unix()).expect("the endpoint should answer");

        let five_hour = utilization.five_hour.expect("a five-hour window");
        assert!(
            (0.0..=100.0).contains(&five_hour.used_percent),
            "utilization should be a percentage, got {}",
            five_hour.used_percent
        );
        eprintln!(
            "five_hour {}% resets_at {:?}",
            five_hour.used_percent, five_hour.resets_at
        );
    }

    #[test]
    fn reads_both_windows_from_a_real_response() {
        let parsed = parse(&response());

        let five_hour = parsed.five_hour.expect("five_hour is present");
        assert_eq!(five_hour.used_percent, 11.0);
        assert_eq!(five_hour.resets_at, Some(1788124200));

        let seven_day = parsed.seven_day.expect("seven_day is present");
        assert_eq!(seven_day.used_percent, 4.0);
        assert_eq!(
            seven_day.resets_at, None,
            "a null reset time is absent, not zero"
        );
    }

    #[test]
    fn a_window_that_does_not_apply_is_absent_rather_than_empty() {
        let mut body = response();
        body["five_hour"] = Value::Null;

        assert!(parse(&body).five_hour.is_none());
    }

    #[test]
    fn an_unrecognised_shape_reports_nothing_rather_than_guessing() {
        // The endpoint is internal and undocumented. If it is renamed or
        // restructured, the honest outcome is no reading at all.
        let renamed = serde_json::json!({
            "five_hour": { "used_percentage": 11.0 },
            "seven_day": { "percent": 4 }
        });

        let parsed = parse(&renamed);
        assert!(parsed.five_hour.is_none());
        assert!(parsed.seven_day.is_none());
    }
}
