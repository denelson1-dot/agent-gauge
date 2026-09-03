//! Claude usage read from the desktop app's own history file.
//!
//! Both other sources need something the user may not have. The status-line
//! capture in [`super::claude`] needs an open terminal session. The endpoint in
//! [`super::claude_usage`] needs a valid Claude Code sign-in — and Claude Code
//! is what refreshes that token, so for anyone working mainly in the desktop
//! app it expires and simply stays expired. The gauge then sits on its last
//! reading indefinitely, which is how a window with no activity in it reads as
//! a confident `0%` hours after the fact.
//!
//! The desktop app polls the same endpoint and writes what it learns here, so
//! this source needs no terminal, no credentials of our own, and no request.
//! While the app is running it is also close to current: samples land every
//! five to fifteen minutes.
//!
//! What it does not carry is reset times — only the two utilisation figures.
//! That is why it is a fallback and not the primary: a reading from here is
//! complete enough to draw a ring, but not to say when that ring empties.
//! [`super::claude`] fills those in from whatever the endpoint last reported,
//! since a reset timestamp stays true regardless of when it was observed.
//!
//! The file belongs to another application and its shape is not documented, so
//! it is read defensively and never written. An unrecognised shape degrades to
//! "no reading" rather than to a confidently wrong number.

use std::fs;

use serde::Deserialize;

use crate::paths;

use super::claude::CapturedWindow;

/// The schema this build knows how to read, from the file's own `version`.
const SUPPORTED_VERSION: u32 = 2;

/// How old the newest sample may be before it stops being worth showing.
///
/// The app samples every five to fifteen minutes while it runs, so an hour
/// absorbs a few missed samples without reaching back to a previous sitting.
/// Beyond that the app has been closed long enough that the number is more
/// likely to mislead than to inform, and saying nothing is the better answer.
///
/// Note this is deliberately longer than the widget's own staleness threshold:
/// a reading between the two is shown, carrying its true age, and marked stale
/// by the renderer rather than hidden.
const TRUSTED_FOR: i64 = 3_600;

pub(super) struct Reading {
    pub(super) five_hour: Option<CapturedWindow>,
    pub(super) seven_day: Option<CapturedWindow>,
    pub(super) observed_at: i64,
}

#[derive(Debug, Deserialize)]
struct History {
    version: u32,
    #[serde(default)]
    samples: Vec<Sample>,
}

#[derive(Debug, Deserialize)]
struct Sample {
    /// Milliseconds, unlike every other timestamp Agent Gauge handles.
    t: i64,
    #[serde(default)]
    u: Utilization,
}

/// Every field optional: the app records what it knows, and a window that does
/// not apply to the account is simply absent.
#[derive(Debug, Default, Deserialize)]
struct Utilization {
    /// `five_hour`, in the endpoint's spelling.
    fh: Option<f64>,
    /// `seven_day`.
    sd: Option<f64>,
}

/// The most recent usable sample, or `None` when the desktop app cannot
/// currently answer.
///
/// Every failure here is a silent `None` rather than a `ProviderFailure`. The
/// desktop app is optional — most installations will not have this file at all
/// — so its absence is not a fault worth reporting against the provider, and
/// the caller has other sources to try.
pub(super) fn read(now: i64) -> Option<Reading> {
    let history = load()?;
    if history.version != SUPPORTED_VERSION {
        return None;
    }
    latest(history.samples, now)
}

/// The newest sample that carries a reading and is recent enough to trust.
///
/// Samples are appended in order, but that is the writer's habit rather than a
/// guarantee we can lean on, so the newest is selected rather than assumed to
/// be last.
fn latest(samples: Vec<Sample>, now: i64) -> Option<Reading> {
    samples
        .into_iter()
        .filter_map(|sample| {
            let observed_at = sample.t.div_euclid(1_000);
            let reading = Reading {
                five_hour: window(sample.u.fh),
                seven_day: window(sample.u.sd),
                observed_at,
            };
            (reading.five_hour.is_some() || reading.seven_day.is_some()).then_some(reading)
        })
        .filter(|reading| is_current(reading.observed_at, now))
        .max_by_key(|reading| reading.observed_at)
}

/// A sample from the future is a clock that has moved, not a reading worth
/// trusting, so the distance is measured in both directions.
fn is_current(observed_at: i64, now: i64) -> bool {
    (now - observed_at).abs() < TRUSTED_FOR
}

fn window(used_percent: Option<f64>) -> Option<CapturedWindow> {
    let used_percent = used_percent?;
    used_percent.is_finite().then_some(CapturedWindow {
        used_percent,
        // The desktop app records utilisation only. See the module comment.
        resets_at: None,
    })
}

fn load() -> Option<History> {
    let bytes = fs::read(paths::claude_desktop_history_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_788_361_363;

    fn parse(value: serde_json::Value) -> Option<History> {
        serde_json::from_value(value).ok()
    }

    fn history(samples: serde_json::Value) -> History {
        parse(json!({ "version": 2, "samples": samples })).expect("valid history")
    }

    #[test]
    fn reads_the_newest_sample() {
        let history = history(json!([
            { "t": (NOW - 900) * 1_000, "u": { "fh": 11, "sd": 12 } },
            { "t": (NOW - 300) * 1_000, "u": { "fh": 13, "sd": 12 } },
        ]));

        let reading = latest(history.samples, NOW).expect("a current reading");

        assert_eq!(reading.five_hour.expect("five hour").used_percent, 13.0);
        assert_eq!(reading.seven_day.expect("seven day").used_percent, 12.0);
        assert_eq!(reading.observed_at, NOW - 300);
    }

    #[test]
    fn prefers_the_newest_sample_regardless_of_position() {
        // Order is the writer's habit, not a guarantee.
        let history = history(json!([
            { "t": (NOW - 300) * 1_000, "u": { "fh": 13 } },
            { "t": (NOW - 900) * 1_000, "u": { "fh": 11 } },
        ]));

        let reading = latest(history.samples, NOW).expect("a current reading");

        assert_eq!(reading.five_hour.expect("five hour").used_percent, 13.0);
    }

    #[test]
    fn a_reading_never_carries_a_reset_time() {
        // The file does not record them; inventing one would be worse than the
        // renderer saying the reset is unavailable.
        let history = history(json!([{ "t": NOW * 1_000, "u": { "fh": 13, "sd": 12 } }]));

        let reading = latest(history.samples, NOW).expect("a current reading");

        assert_eq!(reading.five_hour.expect("five hour").resets_at, None);
        assert_eq!(reading.seven_day.expect("seven day").resets_at, None);
    }

    #[test]
    fn ignores_samples_older_than_the_trust_window() {
        // The steady state for a desktop app that has been closed for a while.
        let history = history(json!([
            { "t": (NOW - TRUSTED_FOR - 1) * 1_000, "u": { "fh": 90, "sd": 90 } },
        ]));

        assert!(latest(history.samples, NOW).is_none());
    }

    #[test]
    fn ignores_samples_from_the_future() {
        let history = history(json!([
            { "t": (NOW + TRUSTED_FOR + 1) * 1_000, "u": { "fh": 90 } },
        ]));

        assert!(latest(history.samples, NOW).is_none());
    }

    #[test]
    fn skips_samples_that_carry_no_window() {
        let history = history(json!([
            { "t": (NOW - 60) * 1_000, "u": {} },
            { "t": (NOW - 600) * 1_000, "u": { "fh": 7 } },
        ]));

        let reading = latest(history.samples, NOW).expect("a current reading");

        assert_eq!(reading.observed_at, NOW - 600);
        assert_eq!(reading.five_hour.expect("five hour").used_percent, 7.0);
    }

    #[test]
    fn keeps_a_window_the_account_does_not_have() {
        // One window absent is normal; it must not discard the other.
        let history = history(json!([{ "t": NOW * 1_000, "u": { "sd": 12 } }]));

        let reading = latest(history.samples, NOW).expect("a current reading");

        assert!(reading.five_hour.is_none());
        assert_eq!(reading.seven_day.expect("seven day").used_percent, 12.0);
    }

    #[test]
    fn rejects_a_non_finite_utilization() {
        assert!(window(Some(f64::NAN)).is_none());
        assert!(window(Some(f64::INFINITY)).is_none());
        assert!(window(None).is_none());
    }

    #[test]
    fn an_empty_history_is_not_a_reading() {
        assert!(latest(history(json!([])).samples, NOW).is_none());
    }

    #[test]
    fn an_unreadable_shape_is_declined() {
        assert!(parse(json!({ "samples": "not a list" })).is_none());
        assert!(parse(json!([])).is_none());
    }
}
