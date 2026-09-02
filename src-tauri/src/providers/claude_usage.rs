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

/// Matches the timeout Claude Code uses for the same request, doubled for the
/// slower networks a desktop widget will sit on. This runs on the provider's
/// own thread, so it delays one refresh and nothing else.
const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Utilization {
    pub(super) five_hour: Option<CapturedWindow>,
    pub(super) seven_day: Option<CapturedWindow>,
}

/// A reading, the moment it was taken — which is not necessarily now — and
/// anything the caller should warn about while showing it.
#[derive(Debug)]
pub(super) struct Reading {
    pub(super) five_hour: Option<CapturedWindow>,
    pub(super) seven_day: Option<CapturedWindow>,
    pub(super) observed_at: i64,
    /// Set when the last attempt failed but an earlier reading survives. The
    /// numbers are still worth showing; the reason they have stopped moving is
    /// worth saying alongside them.
    pub(super) warning: Option<ProviderFailure>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedUsage {
    schema_version: u32,
    /// When the endpoint was last *asked*, whatever came back.
    ///
    /// The floor is measured from here rather than from the last success, so a
    /// failing endpoint is not asked more often than a working one. Measuring
    /// from the last success meant a 429 or an expired sign-in fell back to the
    /// widget's refresh interval and tripled the request rate at exactly the
    /// moment that was least welcome.
    attempted_at: i64,
    observed_at: Option<i64>,
    five_hour: Option<CapturedWindow>,
    seven_day: Option<CapturedWindow>,
    /// Replayed while the floor holds, so a suppressed retry still explains
    /// itself instead of going quiet.
    failure: Option<CachedFailure>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedFailure {
    code: String,
    message: String,
    disconnected: bool,
}

/// The endpoint's view of usage, asked at most once per `poll_interval`
/// seconds — the user's *Backup usage check* setting.
///
/// The floor is deliberately decoupled from the widget's refresh interval,
/// which governs how often the *display* is brought up to date, not how often
/// a provider is allowed to make a network request. Without it, turning the
/// refresh rate up would silently turn the request rate up with it. The cost
/// of a longer floor is bounded staleness; the benefit is a hard ceiling on
/// requests no matter how the widget is configured.
///
/// Every refresh in between is answered from what is already on disk, stamped
/// with when that reading was actually taken rather than when it was served.
/// The widget refreshing more often than this costs nothing and asks nothing of
/// Anthropic.
pub(super) fn read(now: i64, poll_interval: i64) -> Result<Reading, ProviderFailure> {
    let cached = load_cache().filter(is_usable);

    if let Some(cached) = cached
        .as_ref()
        .filter(|cached| within_floor(cached, now, poll_interval))
    {
        return replay(cached);
    }

    match fetch(now) {
        Ok(utilization) => {
            let reading = Reading {
                five_hour: utilization.five_hour,
                seven_day: utilization.seven_day,
                observed_at: now,
                warning: None,
            };
            store(&cached_from(&reading, now, None));
            Ok(reading)
        }
        Err(failure) => {
            let kept = cached.filter(|cached| cached.has_reading());
            store(&failed_attempt(kept.as_ref(), &failure, now));
            match kept {
                Some(cached) => Ok(cached.into_reading(Some(failure))),
                None => Err(failure),
            }
        }
    }
}

/// A cache this build knows how to read. An unknown schema is discarded rather
/// than half-understood, which costs one request and no correctness.
fn is_usable(cached: &CachedUsage) -> bool {
    cached.schema_version == 1
}

fn within_floor(cached: &CachedUsage, now: i64, poll_interval: i64) -> bool {
    now.saturating_sub(cached.attempted_at) < poll_interval
}

/// What to serve without asking the endpoint again.
fn replay(cached: &CachedUsage) -> Result<Reading, ProviderFailure> {
    match (cached.has_reading(), cached.failure.as_ref()) {
        (true, failure) => Ok(cached.clone().into_reading(failure.map(Into::into))),
        (false, Some(failure)) => Err(failure.into()),
        (false, None) => Err(ProviderFailure::disconnected(
            "usage_unavailable",
            "Claude reported no rate-limit windows for this account",
        )),
    }
}

impl CachedUsage {
    fn has_reading(&self) -> bool {
        self.observed_at.is_some() && (self.five_hour.is_some() || self.seven_day.is_some())
    }

    fn into_reading(self, warning: Option<ProviderFailure>) -> Reading {
        Reading {
            observed_at: self.observed_at.unwrap_or(self.attempted_at),
            five_hour: self.five_hour,
            seven_day: self.seven_day,
            warning,
        }
    }
}

impl From<&CachedFailure> for ProviderFailure {
    fn from(failure: &CachedFailure) -> Self {
        Self {
            code: failure.code.clone(),
            message: failure.message.clone(),
            disconnected: failure.disconnected,
        }
    }
}

fn cached_from(
    reading: &Reading,
    attempted_at: i64,
    failure: Option<&ProviderFailure>,
) -> CachedUsage {
    CachedUsage {
        schema_version: 1,
        attempted_at,
        observed_at: Some(reading.observed_at),
        five_hour: reading.five_hour.clone(),
        seven_day: reading.seven_day.clone(),
        failure: failure.map(|failure| CachedFailure {
            code: failure.code.clone(),
            message: failure.message.clone(),
            disconnected: failure.disconnected,
        }),
    }
}

/// Records that the endpoint was asked and did not answer, carrying any earlier
/// reading across so a transient failure does not discard good data.
fn failed_attempt(
    kept: Option<&CachedUsage>,
    failure: &ProviderFailure,
    attempted_at: i64,
) -> CachedUsage {
    CachedUsage {
        schema_version: 1,
        attempted_at,
        observed_at: kept.and_then(|cached| cached.observed_at),
        five_hour: kept.and_then(|cached| cached.five_hour.clone()),
        seven_day: kept.and_then(|cached| cached.seven_day.clone()),
        failure: Some(CachedFailure {
            code: failure.code.clone(),
            message: failure.message.clone(),
            disconnected: failure.disconnected,
        }),
    }
}

fn load_cache() -> Option<CachedUsage> {
    let bytes = fs::read(paths::cache_dir().join(CACHE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best effort. A cache that cannot be written costs an extra request next
/// refresh, which is not worth failing a good reading over.
fn store(cached: &CachedUsage) {
    if let Err(error) = atomic_write_json(&paths::cache_dir().join(CACHE_FILE), cached) {
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

    /// The value the setting defaults to, in the units the floor works in.
    const DEFAULT_POLL_INTERVAL: i64 = crate::model::DEFAULT_CLAUDE_USAGE_POLL_SECONDS as i64;

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

    fn cached(attempted_at: i64) -> CachedUsage {
        CachedUsage {
            schema_version: 1,
            attempted_at,
            observed_at: Some(attempted_at),
            five_hour: Some(CapturedWindow {
                used_percent: 11.0,
                resets_at: Some(9_000),
            }),
            seven_day: None,
            failure: None,
        }
    }

    fn refused() -> ProviderFailure {
        ProviderFailure::disconnected("oauth_expired", "Claude sign-in has expired")
    }

    #[test]
    fn a_reading_inside_the_poll_interval_is_reused_rather_than_refetched() {
        // The widget refreshes every five minutes by default and can be set to
        // every minute. Neither may turn into a request.
        assert!(within_floor(
            &cached(1_000),
            1_000 + DEFAULT_POLL_INTERVAL - 1,
            DEFAULT_POLL_INTERVAL
        ));
    }

    #[test]
    fn the_endpoint_is_asked_again_once_the_interval_has_passed() {
        assert!(!within_floor(
            &cached(1_000),
            1_000 + DEFAULT_POLL_INTERVAL,
            DEFAULT_POLL_INTERVAL
        ));
    }

    #[test]
    fn the_floor_is_whatever_the_user_chose() {
        // A one-minute setting asks again after a minute; a twenty-minute one
        // waits the full twenty.
        assert!(!within_floor(&cached(1_000), 1_060, 60));
        assert!(within_floor(&cached(1_000), 1_060, 1_200));
        assert!(!within_floor(&cached(1_000), 2_200, 1_200));
    }

    #[test]
    fn the_default_request_ceiling_holds_at_six_an_hour() {
        // Stated in the README and worth failing a build over if it drifts.
        assert_eq!(3_600 / DEFAULT_POLL_INTERVAL, 6);
    }

    #[test]
    fn a_failed_attempt_holds_the_floor_just_as_a_successful_one_does() {
        // The bug this guards: measuring the floor from the last success meant
        // a 429 or an expired sign-in fell back to the widget's refresh
        // interval and asked *more* often precisely when it should ask less.
        let attempt = failed_attempt(None, &refused(), 1_000);

        assert!(within_floor(
            &attempt,
            1_000 + DEFAULT_POLL_INTERVAL - 1,
            DEFAULT_POLL_INTERVAL
        ));
        assert!(!within_floor(
            &attempt,
            1_000 + DEFAULT_POLL_INTERVAL,
            DEFAULT_POLL_INTERVAL
        ));
    }

    #[test]
    fn a_failure_carries_an_earlier_reading_across_rather_than_discarding_it() {
        let attempt = failed_attempt(Some(&cached(1_000)), &refused(), 2_000);

        assert!(attempt.has_reading());
        assert_eq!(attempt.observed_at, Some(1_000));
        assert_eq!(
            attempt.attempted_at, 2_000,
            "the floor runs from the attempt, the reading keeps its own age"
        );
    }

    #[test]
    fn a_suppressed_retry_still_shows_the_numbers_and_says_what_is_wrong() {
        let attempt = failed_attempt(Some(&cached(1_000)), &refused(), 2_000);

        let reading = replay(&attempt).expect("an earlier reading should still be served");

        assert_eq!(reading.observed_at, 1_000);
        assert_eq!(reading.five_hour.unwrap().used_percent, 11.0);
        assert_eq!(reading.warning.expect("a warning").code, "oauth_expired");
    }

    #[test]
    fn a_suppressed_retry_with_nothing_to_show_reports_the_reason() {
        let attempt = failed_attempt(None, &refused(), 1_000);

        let failure = replay(&attempt).expect_err("there is nothing to show");

        assert_eq!(failure.code, "oauth_expired");
    }

    #[test]
    fn a_cache_from_an_unknown_schema_is_refetched_rather_than_trusted() {
        let mut future = cached(1_000);
        future.schema_version = 99;

        assert!(!is_usable(&future));
        assert!(is_usable(&cached(1_000)));
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
