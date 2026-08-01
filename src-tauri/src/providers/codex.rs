use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::model::{Balance, ConnectionState, ProviderSnapshot, UsageWindow, WindowDisplay};

use super::{now_unix, ProviderFailure};

const TIMEOUT: Duration = Duration::from_secs(12);

pub fn read() -> Result<ProviderSnapshot, ProviderFailure> {
    let mut child = Command::new("codex")
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderFailure::disconnected("cli_missing", format!("Codex CLI unavailable: {error}"))
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderFailure::new("process_io", "Codex stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProviderFailure::new("process_io", "Codex stderr unavailable"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(8192).read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderFailure::new("process_io", "Codex stdin unavailable"))?;
    for message in [
        json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "agent-gauge", "title": "Agent Gauge", "version": env!("CARGO_PKG_VERSION") }
            }
        }),
        json!({ "method": "initialized", "params": {} }),
        json!({ "id": 2, "method": "account/rateLimits/read", "params": null }),
    ] {
        writeln!(stdin, "{message}")
            .and_then(|_| stdin.flush())
            .map_err(|error| {
                ProviderFailure::new("process_io", format!("Codex request failed: {error}"))
            })?;
    }

    let started = Instant::now();
    let response = loop {
        let remaining = TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderFailure::new(
                "timeout",
                "Codex did not respond in time",
            ));
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_i64) == Some(2) {
                    break value;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(Some(_)) = child.try_wait() {
                    let diagnostic = stderr_reader.join().unwrap_or_default();
                    return Err(classify_exit(&diagnostic));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.wait();
                let diagnostic = stderr_reader.join().unwrap_or_default();
                return Err(classify_exit(&diagnostic));
            }
        }
    };

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    parse_response(&response)
}

fn classify_exit(stderr: &str) -> ProviderFailure {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("login") || lower.contains("auth") {
        ProviderFailure::disconnected("logged_out", "Sign in with the Codex CLI")
    } else {
        ProviderFailure::new("protocol_error", "Codex app-server closed before replying")
    }
}

pub(crate) fn parse_response(response: &Value) -> Result<ProviderSnapshot, ProviderFailure> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex returned an error");
        let lower = message.to_ascii_lowercase();
        return Err(if lower.contains("login") || lower.contains("auth") {
            ProviderFailure::disconnected("logged_out", "Sign in with the Codex CLI")
        } else {
            ProviderFailure::new("protocol_error", message)
        });
    }

    let result = response.get("result").ok_or_else(|| {
        ProviderFailure::new("malformed_response", "Codex response had no result")
    })?;
    let overall = result
        .pointer("/rateLimitsByLimitId/codex")
        .or_else(|| {
            let candidate = result.get("rateLimits")?;
            let id = candidate
                .get("limitId")
                .or_else(|| candidate.get("limit_id"))
                .and_then(Value::as_str);
            (id.is_none() || id == Some("codex")).then_some(candidate)
        })
        .or_else(|| {
            (result.get("primary").is_some() || result.get("secondary").is_some()).then_some(result)
        })
        .ok_or_else(|| {
            ProviderFailure::new(
                "overall_limit_missing",
                "Codex overall usage is unavailable",
            )
        })?;

    let mut windows = Vec::new();
    if let Some(window) = overall.get("primary").filter(|window| !window.is_null()) {
        windows.push(parse_window("primary", window)?);
    }
    if let Some(window) = overall.get("secondary").filter(|window| !window.is_null()) {
        windows.push(parse_window("secondary", window)?);
    }
    windows.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    for window in &mut windows {
        window.display = match window.window_minutes {
            Some(minutes) if minutes >= 1_440 => WindowDisplay::Bar,
            Some(_) => WindowDisplay::Ring,
            None if window.id == "secondary" => WindowDisplay::Bar,
            None => WindowDisplay::Ring,
        };
    }

    let credits = result.get("credits").or_else(|| overall.get("credits"));
    let balances = credits
        .and_then(|credits| credits.get("balance"))
        .and_then(value_as_decimal)
        .map(|amount| {
            vec![Balance {
                id: "credits".into(),
                label: "Credits".into(),
                amount: Some(amount),
                unit: Some("USD".into()),
                known: true,
            }]
        })
        .unwrap_or_default();

    Ok(ProviderSnapshot {
        schema_version: crate::model::SNAPSHOT_SCHEMA_VERSION,
        id: "codex".into(),
        name: "Codex".into(),
        accent: Some("#74a7ff".into()),
        state: ConnectionState::Connected,
        status_message: if windows.is_empty() {
            "Connected; no usage windows reported".into()
        } else {
            "Connected".into()
        },
        observed_at: Some(now_unix()),
        last_attempt_at: None,
        error_code: None,
        windows,
        balances,
        refreshing: false,
    })
}

fn parse_window(id: &str, value: &Value) -> Result<UsageWindow, ProviderFailure> {
    let used = get_number(value, &["usedPercent", "used_percentage", "used_percent"]).ok_or_else(
        || ProviderFailure::new("malformed_response", format!("Codex {id} usage is missing")),
    )?;
    if !used.is_finite() {
        return Err(ProviderFailure::new(
            "malformed_response",
            format!("Codex {id} usage is invalid"),
        ));
    }
    let minutes = get_number(
        value,
        &[
            "windowDurationMins",
            "windowDurationMinutes",
            "window_minutes",
        ],
    )
    .map(|minutes| minutes.max(0.0) as u64);
    Ok(UsageWindow {
        id: id.into(),
        label: minutes.map(window_label).unwrap_or_else(|| title(id)),
        used_percent: used,
        resets_at: value
            .get("resetsAt")
            .or_else(|| value.get("resets_at"))
            .and_then(timestamp),
        window_minutes: minutes,
        display: WindowDisplay::Ring,
    })
}

fn get_number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = value.get(*key)?;
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

fn timestamp(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .map(|time| time.timestamp())
        })
}

fn value_as_decimal(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
        .or_else(|| value.as_u64().map(|number| number.to_string()))
        .or_else(|| value.as_f64().map(|number| number.to_string()))
}

fn window_label(minutes: u64) -> String {
    if minutes.is_multiple_of(10_080) {
        let weeks = minutes / 10_080;
        if weeks == 1 {
            "Weekly".into()
        } else {
            format!("{weeks} weeks")
        }
    } else if minutes.is_multiple_of(1_440) {
        format!("{} day", minutes / 1_440)
    } else if minutes.is_multiple_of(60) {
        format!("{} hour", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

fn title(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_overall_codex_bucket_and_preserves_zero_credit() {
        let response = json!({
            "id": 2,
            "result": {
                "rateLimitsByLimitId": {
                    "codex": {
                        "primary": { "usedPercent": 42.5, "windowDurationMins": 300, "resetsAt": 2_000_000_000 },
                        "secondary": { "usedPercent": 18, "windowDurationMins": 10080 }
                    },
                    "model-x": { "primary": { "usedPercent": 99 } }
                },
                "credits": { "balance": "0" }
            }
        });
        let snapshot = parse_response(&response).unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].label, "5 hour");
        assert_eq!(snapshot.windows[1].label, "Weekly");
        assert_eq!(snapshot.windows[0].display, WindowDisplay::Ring);
        assert_eq!(snapshot.windows[1].display, WindowDisplay::Bar);
        assert_eq!(snapshot.balances[0].amount.as_deref(), Some("0"));
    }

    #[test]
    fn missing_credits_are_not_zero() {
        let response = json!({
            "id": 2,
            "result": { "rateLimits": { "limitId": "codex", "primary": { "usedPercent": 7 } } }
        });
        assert!(parse_response(&response).unwrap().balances.is_empty());
    }
}
