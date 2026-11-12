//! Codex usage, read from its session logs.
//!
//! Codex has no usage endpoint, but after each turn it appends a
//! `token_count` event carrying the account's rate limits to the session's
//! rollout log, `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`:
//!
//! ```json
//! {"timestamp":"2025-09-10T12:34:56.789Z","type":"event_msg",
//!  "payload":{"type":"token_count","info":{...},
//!   "rate_limits":{"primary":{"used_percent":42.0,"window_minutes":300,"resets_at":1757525696},
//!                  "secondary":{"used_percent":67.0,"window_minutes":10080,"resets_in_seconds":277200}}}}
//! ```
//!
//! `primary` is the session window and `secondary` the weekly one. Older
//! Codex versions give `resets_in_seconds` (from the event's timestamp)
//! instead of `resets_at` (Unix seconds), and put the event under `msg`
//! rather than `payload`; both are read. The numbers are as fresh as the last
//! Codex turn, which [`UsageSnapshot::observed_at`] records.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::{Provider, UsageSnapshot, UsageWindow, WindowKind, parse_rfc3339};

/// How many of the most recently written logs to look through for limits.
const LOGS_TO_TRY: usize = 5;

/// Codex's home: `$CODEX_HOME`, or `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => dirs::home_dir().map(|home| home.join(".codex")),
    }
}

/// The latest limits found in the session logs under `home`, from the most
/// recently written logs. `None` when there are no logs or none has limits.
pub fn latest_snapshot(home: &Path) -> io::Result<Option<UsageSnapshot>> {
    let sessions = home.join("sessions");
    let mut logs = Vec::new();
    match collect_logs(&sessions, 0, &mut logs) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    }
    // Resumed sessions append to their original log, so the newest log is
    // the most recently modified one, not the one in the newest folder.
    logs.sort_by_key(|log| std::cmp::Reverse(log.0));
    for (_, path) in logs.into_iter().take(LOGS_TO_TRY) {
        match read_log(&path) {
            Ok(Some(snapshot)) => return Ok(Some(snapshot)),
            Ok(None) => {}
            Err(e) => tracing::debug!("reading {}: {e}", path.display()),
        }
    }
    Ok(None)
}

/// Collects `rollout-*.jsonl` files with their modification times, down the
/// `YYYY/MM/DD` folders.
fn collect_logs(dir: &Path, depth: u32, out: &mut Vec<(SystemTime, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if depth < 3 {
                // An unreadable folder only hides its own logs.
                let _ = collect_logs(&path, depth + 1, out);
            }
        } else if is_rollout_log(&path)
            && let Ok(modified) = entry.metadata().and_then(|m| m.modified())
        {
            out.push((modified, path));
        }
    }
    Ok(())
}

fn is_rollout_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

/// The last limits in one log. Events without a timestamp are dated by the
/// file's modification time.
pub fn read_log(path: &Path) -> io::Result<Option<UsageSnapshot>> {
    let modified = fs::metadata(path)?.modified()?;
    let bytes = fs::read(path)?;
    Ok(parse_log(&String::from_utf8_lossy(&bytes), modified))
}

/// The last limits in a log's text.
pub fn parse_log(text: &str, fallback_time: SystemTime) -> Option<UsageSnapshot> {
    text.lines()
        .rev()
        .find_map(|line| parse_line(line, fallback_time))
}

/// The limits in one log line, if it is a `token_count` event that has them.
pub fn parse_line(line: &str, fallback_time: SystemTime) -> Option<UsageSnapshot> {
    // Most lines are messages; skip them before parsing any JSON.
    if !line.contains("rate_limits") {
        return None;
    }
    let root: Value = serde_json::from_str(line).ok()?;
    let event = root
        .get("payload")
        .or_else(|| root.get("msg"))
        .unwrap_or(&root);
    if event.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let limits = event.get("rate_limits")?.as_object()?;
    let observed_at = root
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339)
        .unwrap_or(fallback_time);

    let windows: Vec<UsageWindow> = [
        ("primary", WindowKind::Session),
        ("secondary", WindowKind::Week),
    ]
    .into_iter()
    .filter_map(|(name, default)| window(limits.get(name)?, default, observed_at))
    .collect();
    if windows.is_empty() {
        return None;
    }
    let plan = limits
        .get("plan_type")
        .or_else(|| event.get("plan_type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(UsageSnapshot {
        provider: Provider::Codex,
        plan,
        windows,
        observed_at,
    })
}

fn window(value: &Value, default: WindowKind, observed_at: SystemTime) -> Option<UsageWindow> {
    let used = value.get("used_percent")?.as_f64()? as f32;
    let minutes = value.get("window_minutes").and_then(Value::as_u64);
    let resets_at = match value.get("resets_at") {
        Some(Value::String(s)) => parse_rfc3339(s),
        Some(at) => at
            .as_f64()
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs_f64(secs)),
        None => None,
    }
    .or_else(|| {
        let secs = value.get("resets_in_seconds")?.as_u64()?;
        Some(observed_at + Duration::from_secs(secs))
    });
    Some(UsageWindow {
        kind: WindowKind::from_minutes(minutes, default),
        scope: None,
        used,
        resets_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2025-09-10T12:34:56Z.
    const T: u64 = 1_757_507_696;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    const CURRENT: &str = r#"{"timestamp":"2025-09-10T12:34:56Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10}},"rate_limits":{"primary":{"used_percent":42.0,"window_minutes":300,"resets_at":1757525696},"secondary":{"used_percent":67.5,"window_minutes":10080,"resets_at":1757784896},"plan_type":"plus"}}}"#;

    #[test]
    fn reads_the_current_format() {
        let s = parse_line(CURRENT, at(0)).unwrap();
        assert_eq!(s.provider, Provider::Codex);
        assert_eq!(s.plan.as_deref(), Some("plus"));
        assert_eq!(s.observed_at, at(T));
        let session = s.session().unwrap();
        assert_eq!(session.used, 42.0);
        assert_eq!(session.resets_at, Some(at(T + 5 * 3600)));
        let week = s.week().unwrap();
        assert_eq!(week.used, 67.5);
        assert_eq!(week.resets_at, Some(at(T + 77 * 3600)));
    }

    #[test]
    fn reads_the_older_format() {
        // Event under `msg`, resets relative to the event.
        let line = r#"{"timestamp":"2025-09-10T12:34:56.000Z","msg":{"type":"token_count","rate_limits":{"primary":{"used_percent":5,"window_minutes":300,"resets_in_seconds":600},"secondary":{"used_percent":1.5,"window_minutes":10080,"resets_in_seconds":86400}}}}"#;
        let s = parse_line(line, at(0)).unwrap();
        assert_eq!(s.plan, None);
        assert_eq!(s.session().unwrap().resets_at, Some(at(T + 600)));
        assert_eq!(s.week().unwrap().used, 1.5);
        assert_eq!(s.week().unwrap().resets_at, Some(at(T + 86_400)));
    }

    #[test]
    fn missing_fields_fall_back() {
        // No timestamp, no window lengths, only a primary window.
        let line = r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":10,"resets_in_seconds":60},"secondary":null}}"#;
        let s = parse_line(line, at(T)).unwrap();
        assert_eq!(s.observed_at, at(T));
        assert_eq!(s.windows.len(), 1);
        assert_eq!(s.session().unwrap().resets_at, Some(at(T + 60)));
        assert_eq!(s.week(), None);
    }

    #[test]
    fn other_window_lengths_are_kept() {
        let line = r#"{"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":3,"window_minutes":480}}}}"#;
        let s = parse_line(line, at(T)).unwrap();
        assert_eq!(s.windows[0].kind, WindowKind::Other { minutes: 480 });
        assert_eq!(s.windows[0].resets_at, None);
    }

    #[test]
    fn ignores_lines_without_limits() {
        for line in [
            "",
            "not json rate_limits",
            r#"{"type":"response_item","payload":{"type":"message","content":"rate_limits"}}"#,
            r#"{"payload":{"type":"token_count","rate_limits":null}}"#,
            r#"{"payload":{"type":"token_count","rate_limits":{}}}"#,
            r#"{"payload":{"type":"token_count","rate_limits":{"primary":{"window_minutes":300}}}}"#,
        ] {
            assert_eq!(parse_line(line, at(T)), None, "{line}");
        }
    }

    #[test]
    fn a_log_gives_its_last_limits() {
        let older = CURRENT
            .replace("42.0", "11.0")
            .replace("12:34:56Z", "12:00:00Z");
        let log = format!(
            "{older}\n{CURRENT}\n{}\n",
            r#"{"timestamp":"2025-09-10T12:35:00Z","type":"response_item","payload":{"type":"message"}}"#
        );
        let s = parse_log(&log, at(0)).unwrap();
        assert_eq!(s.session().unwrap().used, 42.0);
        assert_eq!(parse_log("{}\n\n", at(0)), None);
    }

    /// A temporary Codex home, removed on drop.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(name: &str) -> TempHome {
            let dir =
                std::env::temp_dir().join(format!("lohtty-usage-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempHome(dir)
        }

        fn log(&self, day: &str, name: &str, text: &str, modified: SystemTime) {
            let dir = self.0.join("sessions").join(day);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            fs::write(&path, text).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn finds_the_most_recently_written_log() {
        let home = TempHome::new("latest");
        assert_eq!(latest_snapshot(&home.0).unwrap(), None);

        let resumed = CURRENT.replace("42.0", "77.0");
        // An old session, resumed: written last although in an older folder.
        home.log("2025/09/01", "rollout-a.jsonl", &resumed, at(T + 100));
        home.log("2025/09/10", "rollout-b.jsonl", CURRENT, at(T + 50));
        // Newest, but no limits yet: skipped.
        home.log("2025/09/10", "rollout-c.jsonl", "{}\n", at(T + 200));
        // Not a rollout log.
        home.log("2025/09/10", "notes.jsonl", CURRENT, at(T + 300));

        let s = latest_snapshot(&home.0).unwrap().unwrap();
        assert_eq!(s.session().unwrap().used, 77.0);
    }

    #[test]
    fn a_missing_sessions_folder_is_no_data() {
        let home = TempHome::new("empty");
        fs::remove_dir_all(&home.0).unwrap();
        assert_eq!(latest_snapshot(&home.0).unwrap(), None);
    }
}
