//! The presentation model for the widget surface.
//!
//! Agent Gauge paints the widget two different ways. On Linux it draws directly
//! with GTK/Cairo, because WebKitGTK could not be relied on for a transparent
//! surface. On Windows it renders the React widget in WebView2, which handles
//! transparency correctly and saves an entire second renderer.
//!
//! Two painters is a defensible trade. Two sets of *rules* is not. Before this
//! module existed the Cairo renderer and the React renderer each decided
//! independently what a percentage looked like, when a window had rolled over,
//! and what a provider's status said — and they had already drifted apart in
//! ways nobody could see, because the React widget is hidden on Linux:
//!
//! | Rule            | Cairo said            | React said        |
//! |-----------------|-----------------------|-------------------|
//! | `42.02` percent | `42%`                 | `42.0%`           |
//! | 3h even reset   | `Resets in 3h 0m`     | `Resets in 3h`    |
//! | fresh provider  | `Connected`           | `Updated 5m ago`  |
//!
//! So every rule lives here, once, and both painters consume the result. A
//! painter's only remaining job is to put the given strings and fractions on
//! screen. Adding a third platform should not require re-deriving any of this.
//!
//! Kept deliberately free of Tauri and drawing types so it can be unit-tested
//! on any platform, which is what makes the Windows renderer verifiable from a
//! Linux checkout.

use serde::{Deserialize, Serialize};

use crate::{
    model::{Balance, ConnectionState, ProviderSnapshot, Theme, UsageWindow, WindowDisplay},
    window::{DisplayMode, WidgetState},
};

/// A provider is considered stale once its reading is this old.
const STALE_AFTER_SECONDS: i64 = 1_800;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WidgetView {
    pub theme: Theme,
    pub mode: DisplayMode,
    pub locked: bool,
    /// Shown in the widget's top-right corner.
    pub mode_label: String,
    /// Present only when there is nothing to show, so a painter never has to
    /// decide what an empty widget says.
    pub empty: Option<NoticeView>,
    pub providers: Vec<ProviderView>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct NoticeView {
    pub title: String,
    pub detail: String,
}

/// How a provider's freshness should read. Separate from `status_label` because
/// the two painters style it differently — a colour in Cairo, a CSS class in
/// React — while agreeing on which case applies.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StatusTone {
    Fresh,
    Stale,
    Waiting,
    Error,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub accent: Option<String>,
    pub status_label: String,
    pub tone: StatusTone,
    pub refreshing: bool,
    /// Shown instead of metrics when the provider has no windows yet.
    pub notice: Option<NoticeView>,
    pub windows: Vec<WindowView>,
    /// Only balances the provider actually reported. An unknown balance is
    /// omitted rather than rendered as zero.
    pub balances: Vec<BalanceView>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WindowView {
    pub id: String,
    pub label: String,
    pub display: WindowDisplay,
    /// Percentage to draw, already rolled over and clamped to `0..=100`.
    /// Painters use this for geometry and must not re-derive it.
    pub fill: f64,
    pub percent_label: String,
    pub primary: String,
    pub secondary: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BalanceView {
    pub id: String,
    pub label: String,
    pub amount: String,
}

/// Builds the view from the providers the user has chosen to see, in their
/// chosen order.
pub fn widget_view(
    settings_theme: Theme,
    provider_order: &[String],
    disabled_providers: &[String],
    snapshots: Vec<ProviderSnapshot>,
    window: &WidgetState,
    now: i64,
) -> WidgetView {
    let mut visible: Vec<ProviderSnapshot> = snapshots
        .into_iter()
        .filter(|provider| {
            provider.state != ConnectionState::Disabled
                && provider.state != ConnectionState::Untrusted
                && !disabled_providers.iter().any(|id| id == &provider.id)
        })
        .collect();

    visible.sort_by_key(|provider| {
        provider_order
            .iter()
            .position(|id| id == &provider.id)
            .unwrap_or(usize::MAX)
    });

    let empty = visible.is_empty().then(|| NoticeView {
        title: "No trackers enabled".into(),
        detail: "Open Settings from the tray to choose providers.".into(),
    });

    WidgetView {
        theme: settings_theme,
        mode: window.mode,
        locked: window.locked,
        mode_label: match window.mode {
            DisplayMode::Pinned => "PINNED".into(),
            DisplayMode::Desktop => "DESKTOP".into(),
        },
        empty,
        providers: visible
            .iter()
            .map(|provider| provider_view(provider, now))
            .collect(),
    }
}

pub fn provider_view(provider: &ProviderSnapshot, now: i64) -> ProviderView {
    ProviderView {
        id: provider.id.clone(),
        name: provider.name.clone(),
        accent: provider.accent.clone(),
        status_label: status_label(provider, now),
        tone: status_tone(provider, now),
        refreshing: provider.refreshing,
        notice: provider.windows.is_empty().then(|| NoticeView {
            title: provider.status_message.clone(),
            detail: if provider.id == "claude" {
                "Connect capture in Settings, then use Claude Code normally.".into()
            } else {
                "Open Settings for connection details.".into()
            },
        }),
        windows: provider
            .windows
            .iter()
            .map(|window| window_view(window, now))
            .collect(),
        balances: provider.balances.iter().filter_map(balance_view).collect(),
        warning: provider
            .error_code
            .is_some()
            .then(|| provider.status_message.clone()),
    }
}

pub fn window_view(window: &UsageWindow, now: i64) -> WindowView {
    let rolled_over = window.rolled_over(now);
    let used = window.effective_used_percent(now);

    WindowView {
        id: window.id.clone(),
        label: window.label.clone(),
        display: window.display,
        fill: used.clamp(0.0, 100.0),
        percent_label: percent(used),
        primary: if rolled_over {
            "Window reset".into()
        } else {
            reset_relative(window.resets_at, now)
        },
        secondary: if rolled_over {
            "Awaiting new activity".into()
        } else {
            reset_absolute(window.resets_at)
        },
    }
}

fn balance_view(balance: &Balance) -> Option<BalanceView> {
    // An unknown balance is genuinely unknown. Agent Gauge never substitutes a
    // zero for one a provider did not report, so it is dropped rather than
    // shown with a placeholder.
    if !balance.known {
        return None;
    }
    let amount = match (&balance.amount, balance.unit.as_deref()) {
        (Some(amount), Some("USD")) => format!("${amount}"),
        (Some(amount), Some(unit)) => format!("{amount} {unit}"),
        (Some(amount), None) => amount.clone(),
        (None, _) => return None,
    };
    Some(BalanceView {
        id: balance.id.clone(),
        label: balance.label.clone(),
        amount,
    })
}

fn status_label(provider: &ProviderSnapshot, now: i64) -> String {
    if provider.refreshing {
        return "Refreshing".into();
    }
    match provider.state {
        ConnectionState::Connected if is_stale(provider, now) => "Stale".into(),
        ConnectionState::Connected => "Connected".into(),
        ConnectionState::Waiting => "Waiting".into(),
        ConnectionState::Disconnected => "Disconnected".into(),
        ConnectionState::Error => "Error".into(),
        ConnectionState::Disabled => "Disabled".into(),
        ConnectionState::Untrusted => "Untrusted".into(),
    }
}

fn status_tone(provider: &ProviderSnapshot, now: i64) -> StatusTone {
    match provider.state {
        ConnectionState::Error | ConnectionState::Disconnected => StatusTone::Error,
        ConnectionState::Waiting | ConnectionState::Untrusted | ConnectionState::Disabled => {
            StatusTone::Waiting
        }
        ConnectionState::Connected if is_stale(provider, now) => StatusTone::Stale,
        ConnectionState::Connected => StatusTone::Fresh,
    }
}

fn is_stale(provider: &ProviderSnapshot, now: i64) -> bool {
    provider
        .observed_at
        .is_some_and(|observed| now - observed > STALE_AFTER_SECONDS)
}

/// Formats a percentage, hiding a decimal that would only add noise.
fn percent(value: f64) -> String {
    if value.fract().abs() < 0.05 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

fn reset_relative(timestamp: Option<i64>, now: i64) -> String {
    let Some(timestamp) = timestamp else {
        return "Reset unavailable".into();
    };
    let seconds = timestamp - now;
    if seconds <= 0 {
        return "Reset due".into();
    }
    // Rounded up, so a window with 30 seconds left reads "1m" rather than "0m".
    let minutes = (seconds + 59) / 60;
    if minutes < 60 {
        format!("Resets in {minutes}m")
    } else if minutes < 2_880 {
        format!("Resets in {}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("Resets in {}d {}h", minutes / 1_440, (minutes % 1_440) / 60)
    }
}

fn reset_absolute(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%a %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| "Time unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SNAPSHOT_SCHEMA_VERSION;

    const NOW: i64 = 1_700_000_000;

    fn window(used_percent: f64, resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            id: "five-hour".into(),
            label: "5 hour".into(),
            used_percent,
            resets_at,
            window_minutes: Some(300),
            display: WindowDisplay::Ring,
        }
    }

    fn provider(state: ConnectionState, observed_at: Option<i64>) -> ProviderSnapshot {
        ProviderSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            id: "codex".into(),
            name: "Codex".into(),
            accent: None,
            state,
            status_message: "message".into(),
            observed_at,
            last_attempt_at: None,
            error_code: None,
            windows: Vec::new(),
            balances: Vec::new(),
            refreshing: false,
        }
    }

    #[test]
    fn a_live_window_presents_what_the_provider_reported() {
        let view = window_view(&window(42.5, Some(NOW + 3 * 3600)), NOW);

        assert_eq!(view.fill, 42.5);
        assert_eq!(view.percent_label, "42.5%");
        assert_eq!(view.primary, "Resets in 3h 0m");
    }

    #[test]
    fn a_rolled_over_window_does_not_carry_its_old_usage_forward() {
        // The captured percent describes the window that ended. Showing it
        // would claim usage the user has not made.
        let view = window_view(&window(97.0, Some(NOW - 1)), NOW);

        assert_eq!(view.fill, 0.0);
        assert_eq!(view.percent_label, "0%");
        assert_eq!(view.primary, "Window reset");
        assert_eq!(view.secondary, "Awaiting new activity");
    }

    #[test]
    fn a_window_without_a_reset_time_keeps_its_usage_and_says_so() {
        let view = window_view(&window(61.0, None), NOW);

        assert_eq!(view.fill, 61.0);
        assert_eq!(view.primary, "Reset unavailable");
        assert_eq!(view.secondary, "Time unavailable");
    }

    #[test]
    fn out_of_range_usage_is_clamped_for_drawing_but_reported_honestly() {
        // A provider reporting over 100% should not paint outside the ring,
        // but the label should still say what was reported.
        let view = window_view(&window(140.0, None), NOW);
        assert_eq!(view.fill, 100.0);
        assert_eq!(view.percent_label, "140%");
    }

    #[test]
    fn percentages_hide_a_decimal_that_would_only_add_noise() {
        // Guards the exact case where the two renderers used to disagree:
        // Cairo rounded 42.02 to "42%", React rendered "42.0%".
        assert_eq!(percent(42.0), "42%");
        assert_eq!(percent(42.02), "42%");
        assert_eq!(percent(42.5), "42.5%");
        assert_eq!(percent(0.0), "0%");
    }

    #[test]
    fn relative_resets_round_up_and_change_units() {
        assert_eq!(reset_relative(Some(NOW + 30), NOW), "Resets in 1m");
        assert_eq!(reset_relative(Some(NOW + 59 * 60), NOW), "Resets in 59m");
        assert_eq!(reset_relative(Some(NOW + 3 * 3600), NOW), "Resets in 3h 0m");
        assert_eq!(
            reset_relative(Some(NOW + 3 * 3600 + 14 * 60), NOW),
            "Resets in 3h 14m"
        );
        assert_eq!(
            reset_relative(Some(NOW + 4 * 86_400), NOW),
            "Resets in 4d 0h"
        );
        assert_eq!(reset_relative(Some(NOW), NOW), "Reset due");
        assert_eq!(reset_relative(None, NOW), "Reset unavailable");
    }

    #[test]
    fn a_stale_reading_is_labelled_as_stale() {
        let fresh = provider(ConnectionState::Connected, Some(NOW - 60));
        let stale = provider(
            ConnectionState::Connected,
            Some(NOW - STALE_AFTER_SECONDS - 1),
        );

        assert_eq!(status_label(&fresh, NOW), "Connected");
        assert_eq!(status_tone(&fresh, NOW), StatusTone::Fresh);
        assert_eq!(status_label(&stale, NOW), "Stale");
        assert_eq!(status_tone(&stale, NOW), StatusTone::Stale);
    }

    #[test]
    fn refreshing_takes_precedence_over_the_underlying_state() {
        let mut provider = provider(ConnectionState::Error, None);
        provider.refreshing = true;
        assert_eq!(status_label(&provider, NOW), "Refreshing");
    }

    #[test]
    fn unknown_balances_are_omitted_rather_than_shown_as_zero() {
        let unknown = Balance {
            id: "credits".into(),
            label: "Credits".into(),
            amount: None,
            unit: Some("USD".into()),
            known: false,
        };
        assert_eq!(balance_view(&unknown), None);

        // A reported balance of zero is real data and must survive.
        let zero = Balance {
            id: "credits".into(),
            label: "Credits".into(),
            amount: Some("0".into()),
            unit: Some("USD".into()),
            known: true,
        };
        assert_eq!(balance_view(&zero).unwrap().amount, "$0");
    }

    #[test]
    fn balances_render_their_unit() {
        let make = |unit: Option<&str>| Balance {
            id: "credits".into(),
            label: "Credits".into(),
            amount: Some("13.72".into()),
            unit: unit.map(Into::into),
            known: true,
        };
        assert_eq!(balance_view(&make(Some("USD"))).unwrap().amount, "$13.72");
        assert_eq!(
            balance_view(&make(Some("credits"))).unwrap().amount,
            "13.72 credits"
        );
        assert_eq!(balance_view(&make(None)).unwrap().amount, "13.72");
    }

    #[test]
    fn hidden_and_disabled_providers_are_left_out_in_the_users_order() {
        let mut codex = provider(ConnectionState::Connected, Some(NOW));
        codex.id = "codex".into();
        let mut claude = provider(ConnectionState::Connected, Some(NOW));
        claude.id = "claude".into();
        let mut untrusted = provider(ConnectionState::Untrusted, None);
        untrusted.id = "sketchy".into();
        let mut muted = provider(ConnectionState::Connected, Some(NOW));
        muted.id = "muted".into();

        let view = widget_view(
            Theme::Signal,
            &["claude".into(), "codex".into()],
            &["muted".into()],
            vec![codex, claude, untrusted, muted],
            &WidgetState::default(),
            NOW,
        );

        let ids: Vec<_> = view.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["claude", "codex"]);
        assert!(view.empty.is_none());
    }

    #[test]
    fn an_empty_widget_explains_itself() {
        let view = widget_view(
            Theme::Glass,
            &[],
            &[],
            Vec::new(),
            &WidgetState::default(),
            NOW,
        );

        assert!(view.providers.is_empty());
        assert_eq!(view.empty.unwrap().title, "No trackers enabled");
    }

    #[test]
    fn mode_label_matches_the_display_mode() {
        let pinned_state = WidgetState {
            mode: DisplayMode::Pinned,
            ..Default::default()
        };
        let pinned = widget_view(Theme::Glass, &[], &[], Vec::new(), &pinned_state, NOW);
        assert_eq!(pinned.mode_label, "PINNED");

        let desktop_state = WidgetState {
            mode: DisplayMode::Desktop,
            ..Default::default()
        };
        let desktop = widget_view(Theme::Glass, &[], &[], Vec::new(), &desktop_state, NOW);
        assert_eq!(desktop.mode_label, "DESKTOP");
    }
}
