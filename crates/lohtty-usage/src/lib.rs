//! Usage limits of coding agents: how much of the current session window and
//! of the week is used, and when each resets.
//!
//! Each provider module turns its agent's data into a [`UsageSnapshot`]:
//! - [`codex`] reads the rate limits Codex writes to its session logs.
//!
//! Nothing here touches the network or gpui, so it is all unit tested.

pub mod codex;
mod time;

use std::time::{Duration, SystemTime};

pub use time::parse_rfc3339;

/// The agent a snapshot is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
        }
    }
}

/// What a limit window covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowKind {
    /// The rolling session window (5 hours for both Claude and Codex).
    Session,
    /// The weekly window.
    Week,
    /// Any other length, in minutes.
    Other { minutes: u64 },
}

impl WindowKind {
    /// Classifies a window by its length; `None` falls back to `default`.
    pub fn from_minutes(minutes: Option<u64>, default: WindowKind) -> WindowKind {
        match minutes {
            Some(300) => WindowKind::Session,
            Some(10_080) => WindowKind::Week,
            Some(minutes) => WindowKind::Other { minutes },
            None => default,
        }
    }
}

/// How close a window is to its limit, for coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Below 70 %.
    Normal,
    /// 70 % to below 90 %.
    Warn,
    /// 90 % and above.
    Critical,
}

impl Level {
    pub fn of(used_percent: f32) -> Level {
        if used_percent >= 90.0 {
            Level::Critical
        } else if used_percent >= 70.0 {
            Level::Warn
        } else {
            Level::Normal
        }
    }
}

/// One limit window: its use and when it resets.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    pub kind: WindowKind,
    /// A narrower scope of the window, such as a model ("Opus").
    pub scope: Option<String>,
    /// Percent used, 0 to 100, as last reported.
    pub used: f32,
    /// When the window resets, if known.
    pub resets_at: Option<SystemTime>,
}

impl UsageWindow {
    /// "Session", "Week", "Week Opus", "8 h", "2 d".
    pub fn label(&self) -> String {
        let base = match &self.kind {
            WindowKind::Session => "Session".to_owned(),
            WindowKind::Week => "Week".to_owned(),
            WindowKind::Other { minutes } if minutes % 1440 == 0 => format!("{} d", minutes / 1440),
            WindowKind::Other { minutes } if minutes % 60 == 0 => format!("{} h", minutes / 60),
            WindowKind::Other { minutes } => format!("{minutes} min"),
        };
        match &self.scope {
            Some(scope) => format!("{base} {scope}"),
            None => base,
        }
    }

    /// Whether the reset time has passed since the use was reported.
    pub fn has_reset(&self, now: SystemTime) -> bool {
        self.resets_at.is_some_and(|at| at <= now)
    }

    /// The use now: 0 once the window has reset, as the report predates it.
    pub fn used_at(&self, now: SystemTime) -> f32 {
        if self.has_reset(now) {
            0.0
        } else {
            self.used.clamp(0.0, 100.0)
        }
    }

    /// Time left until the reset; `None` if unknown or already past.
    pub fn remaining(&self, now: SystemTime) -> Option<Duration> {
        self.resets_at?
            .duration_since(now)
            .ok()
            .filter(|left| !left.is_zero())
    }
}

/// Everything known about one provider's limits at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub provider: Provider,
    /// The subscription, such as "Max" or "plus", if reported.
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    /// When these numbers were reported (for Codex, the agent's last turn).
    pub observed_at: SystemTime,
}

impl UsageSnapshot {
    /// The session window, if reported.
    pub fn session(&self) -> Option<&UsageWindow> {
        self.unscoped(&WindowKind::Session)
    }

    /// The weekly window covering all models, if reported.
    pub fn week(&self) -> Option<&UsageWindow> {
        self.unscoped(&WindowKind::Week)
    }

    fn unscoped(&self, kind: &WindowKind) -> Option<&UsageWindow> {
        self.windows
            .iter()
            .find(|w| &w.kind == kind && w.scope.is_none())
    }
}

/// A time left, for people: "< 1 min", "47 min", "2 h 13 min", "3 d 4 h".
/// Rounds down, like a countdown.
pub fn humanize_remaining(left: Duration) -> String {
    let minutes = left.as_secs() / 60;
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (days, hours, mins) {
        (0, 0, 0) => "< 1 min".to_owned(),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, _) => format!("{d} d"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

/// A time since, for people: "just now", "40 s ago", "12 min ago", "3 h ago",
/// "2 d ago".
pub fn humanize_ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..5 => "just now".to_owned(),
        5..60 => format!("{secs} s ago"),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60;
    const HOUR: u64 = 3600;
    const DAY: u64 = 86_400;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn remaining_is_humanized() {
        assert_eq!(humanize_remaining(secs(0)), "< 1 min");
        assert_eq!(humanize_remaining(secs(59)), "< 1 min");
        assert_eq!(humanize_remaining(secs(MIN)), "1 min");
        assert_eq!(humanize_remaining(secs(47 * MIN + 59)), "47 min");
        assert_eq!(humanize_remaining(secs(2 * HOUR)), "2 h");
        assert_eq!(
            humanize_remaining(secs(2 * HOUR + 13 * MIN + 30)),
            "2 h 13 min"
        );
        assert_eq!(humanize_remaining(secs(DAY - 1)), "23 h 59 min");
        assert_eq!(humanize_remaining(secs(3 * DAY + 20 * MIN)), "3 d");
        assert_eq!(
            humanize_remaining(secs(3 * DAY + 4 * HOUR + 59 * MIN)),
            "3 d 4 h"
        );
    }

    #[test]
    fn ago_is_humanized() {
        assert_eq!(humanize_ago(secs(2)), "just now");
        assert_eq!(humanize_ago(secs(40)), "40 s ago");
        assert_eq!(humanize_ago(secs(12 * MIN + 5)), "12 min ago");
        assert_eq!(humanize_ago(secs(3 * HOUR)), "3 h ago");
        assert_eq!(humanize_ago(secs(2 * DAY + HOUR)), "2 d ago");
    }

    #[test]
    fn levels_follow_thresholds() {
        assert_eq!(Level::of(0.0), Level::Normal);
        assert_eq!(Level::of(69.9), Level::Normal);
        assert_eq!(Level::of(70.0), Level::Warn);
        assert_eq!(Level::of(89.9), Level::Warn);
        assert_eq!(Level::of(90.0), Level::Critical);
        assert_eq!(Level::of(100.0), Level::Critical);
    }

    #[test]
    fn labels() {
        let window = |kind, scope: Option<&str>| UsageWindow {
            kind,
            scope: scope.map(str::to_owned),
            used: 0.0,
            resets_at: None,
        };
        assert_eq!(window(WindowKind::Session, None).label(), "Session");
        assert_eq!(window(WindowKind::Week, Some("Opus")).label(), "Week Opus");
        assert_eq!(
            window(WindowKind::Other { minutes: 480 }, None).label(),
            "8 h"
        );
        assert_eq!(
            window(WindowKind::Other { minutes: 2880 }, None).label(),
            "2 d"
        );
        assert_eq!(
            window(WindowKind::Other { minutes: 45 }, None).label(),
            "45 min"
        );
    }

    #[test]
    fn window_kind_from_minutes() {
        assert_eq!(
            WindowKind::from_minutes(Some(300), WindowKind::Week),
            WindowKind::Session
        );
        assert_eq!(
            WindowKind::from_minutes(Some(10_080), WindowKind::Session),
            WindowKind::Week
        );
        assert_eq!(
            WindowKind::from_minutes(Some(60), WindowKind::Week),
            WindowKind::Other { minutes: 60 }
        );
        assert_eq!(
            WindowKind::from_minutes(None, WindowKind::Week),
            WindowKind::Week
        );
    }

    #[test]
    fn a_window_past_its_reset_reads_as_unused() {
        let t0 = SystemTime::UNIX_EPOCH + secs(1_000_000);
        let window = UsageWindow {
            kind: WindowKind::Session,
            scope: None,
            used: 80.0,
            resets_at: Some(t0 + secs(HOUR)),
        };
        assert_eq!(window.used_at(t0), 80.0);
        assert_eq!(window.remaining(t0), Some(secs(HOUR)));
        assert!(!window.has_reset(t0));

        let later = t0 + secs(HOUR);
        assert_eq!(window.used_at(later), 0.0);
        assert_eq!(window.remaining(later), None);
        assert!(window.has_reset(later));
    }

    #[test]
    fn used_is_clamped() {
        let window = UsageWindow {
            kind: WindowKind::Week,
            scope: None,
            used: 130.0,
            resets_at: None,
        };
        assert_eq!(window.used_at(SystemTime::UNIX_EPOCH), 100.0);
    }

    #[test]
    fn snapshot_finds_unscoped_windows() {
        let window = |kind, scope: Option<&str>, used| UsageWindow {
            kind,
            scope: scope.map(str::to_owned),
            used,
            resets_at: None,
        };
        let snapshot = UsageSnapshot {
            provider: Provider::Claude,
            plan: None,
            windows: vec![
                window(WindowKind::Week, Some("Opus"), 12.0),
                window(WindowKind::Session, None, 42.0),
                window(WindowKind::Week, None, 67.0),
            ],
            observed_at: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(snapshot.session().map(|w| w.used), Some(42.0));
        assert_eq!(snapshot.week().map(|w| w.used), Some(67.0));
    }
}
