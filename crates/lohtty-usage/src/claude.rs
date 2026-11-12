//! Claude usage, from the endpoint behind Claude Code's `/usage`.
//!
//! `GET https://api.anthropic.com/api/oauth/usage`, authorized with the OAuth
//! token Claude Code keeps (`~/.claude/.credentials.json`, or the macOS
//! Keychain item `Claude Code-credentials`), answers:
//!
//! ```json
//! {"five_hour":{"utilization":42.0,"resets_at":"2025-09-10T17:34:56.123+00:00"},
//!  "seven_day":{"utilization":67.0,"resets_at":"..."},
//!  "seven_day_opus":{"utilization":12.0,"resets_at":"..."}, ...}
//! ```
//!
//! The endpoint is undocumented and may change. lohtty only *reads* the
//! token: it never refreshes it (Claude Code does, and two refreshers would
//! race), so an expired token is reported as such until Claude Code runs.
//! The token is sent to this one URL only, and never logged.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::{Provider, UsageError, UsageSnapshot, UsageWindow, WindowKind, parse_rfc3339};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA: &str = "oauth-2025-04-20";

/// The parts of Claude Code's credentials lohtty uses.
#[derive(Clone, PartialEq)]
pub struct Credentials {
    access_token: String,
    expires_at: Option<SystemTime>,
    /// "max", "pro", ...
    pub subscription: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("subscription", &self.subscription)
            .finish()
    }
}

impl Credentials {
    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }
}

/// Claude Code's folder: `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
pub fn config_dir() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".claude")),
    }
}

/// Reads Claude Code's credentials. `Ok(None)` when Claude Code isn't
/// signed in with a subscription on this machine.
pub fn load_credentials() -> Result<Option<Credentials>, UsageError> {
    let Some(text) = credentials_text()? else {
        return Ok(None);
    };
    parse_credentials(&text)
}

fn credentials_text() -> Result<Option<String>, UsageError> {
    if let Some(dir) = config_dir() {
        match std::fs::read_to_string(dir.join(".credentials.json")) {
            Ok(text) => return Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(UsageError::Credentials(e.to_string())),
        }
    }
    if cfg!(target_os = "macos") {
        return Ok(keychain_text());
    }
    Ok(None)
}

/// The Keychain item Claude Code uses on macOS. Reading it may show the
/// system's permission prompt once.
fn keychain_text() -> Option<String> {
    let user = std::env::var("USER").unwrap_or_default();
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", "Claude Code-credentials"])
        .args(if user.is_empty() {
            vec![]
        } else {
            vec!["-a", &user]
        })
        .arg("-w")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Parses the credentials JSON: `{"claudeAiOauth":{"accessToken":...,
/// "expiresAt":<ms>,"subscriptionType":...}}`. `Ok(None)` without OAuth
/// credentials (API-key users have no subscription limits).
pub fn parse_credentials(text: &str) -> Result<Option<Credentials>, UsageError> {
    let root: Value =
        serde_json::from_str(text).map_err(|e| UsageError::Credentials(e.to_string()))?;
    let Some(oauth) = root.get("claudeAiOauth") else {
        return Ok(None);
    };
    let Some(token) = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    else {
        return Ok(None);
    };
    let expires_at = oauth
        .get("expiresAt")
        .and_then(Value::as_u64)
        .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
    let subscription = oauth
        .get("subscriptionType")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(Some(Credentials {
        access_token: token.to_owned(),
        expires_at,
        subscription,
    }))
}

/// Asks the usage endpoint. Blocking; run it off the UI thread.
pub fn fetch(
    agent: &ureq::Agent,
    credentials: &Credentials,
    now: SystemTime,
) -> Result<UsageSnapshot, UsageError> {
    if credentials.is_expired(now) {
        return Err(UsageError::Expired);
    }
    let mut response = agent
        .get(USAGE_URL)
        .header(
            "Authorization",
            &format!("Bearer {}", credentials.access_token),
        )
        .header("anthropic-beta", BETA)
        .header("Accept", "application/json")
        .header("User-Agent", concat!("lohtty/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| UsageError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    match status {
        200 => {}
        401 | 403 => return Err(UsageError::Unauthorized),
        429 => {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok())
                .map(Duration::from_secs);
            return Err(UsageError::RateLimited { retry_after });
        }
        _ => return Err(UsageError::Status(status)),
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_string()
        .map_err(|e| UsageError::Network(e.to_string()))?;
    parse_usage(&body, credentials.subscription.as_deref(), now)
}

/// The windows in a usage response, in a fixed order: session, week, then
/// the per-model weeks. Windows that are absent or `null` are skipped.
pub fn parse_usage(
    body: &str,
    subscription: Option<&str>,
    now: SystemTime,
) -> Result<UsageSnapshot, UsageError> {
    let root: Value = serde_json::from_str(body).map_err(|e| UsageError::Parse(e.to_string()))?;
    let root = root
        .as_object()
        .ok_or_else(|| UsageError::Parse("not an object".to_owned()))?;
    const WINDOWS: [(&str, WindowKind, Option<&str>); 4] = [
        ("five_hour", WindowKind::Session, None),
        ("seven_day", WindowKind::Week, None),
        ("seven_day_opus", WindowKind::Week, Some("Opus")),
        ("seven_day_sonnet", WindowKind::Week, Some("Sonnet")),
    ];
    let windows: Vec<UsageWindow> = WINDOWS
        .into_iter()
        .filter_map(|(key, kind, scope)| {
            let window = root.get(key)?;
            let used = window.get("utilization")?.as_f64()? as f32;
            let resets_at = window
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339);
            Some(UsageWindow {
                kind,
                scope: scope.map(str::to_owned),
                used,
                resets_at,
            })
        })
        .collect();
    if windows.is_empty() {
        return Err(UsageError::Parse("no usage windows".to_owned()));
    }
    Ok(UsageSnapshot {
        provider: Provider::Claude,
        plan: subscription.map(plan_name),
        windows,
        observed_at: now,
    })
}

/// "max" → "Max".
fn plan_name(subscription: &str) -> String {
    let mut chars = subscription.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2025-09-10T12:34:56Z.
    const T: u64 = 1_757_507_696;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn reads_credentials() {
        let text = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-x","refreshToken":"r","expiresAt":1757507696000,"scopes":["user:inference"],"subscriptionType":"max"}}"#;
        let c = parse_credentials(text).unwrap().unwrap();
        assert_eq!(c.access_token, "sk-ant-oat01-x");
        assert_eq!(c.expires_at, Some(at(T)));
        assert_eq!(c.subscription.as_deref(), Some("max"));
        assert!(!c.is_expired(at(T - 1)));
        assert!(c.is_expired(at(T)));
        // The token never shows in debug output.
        assert!(!format!("{c:?}").contains("sk-ant"));
    }

    #[test]
    fn no_oauth_is_no_credentials() {
        assert_eq!(parse_credentials("{}").unwrap(), None);
        assert_eq!(
            parse_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#).unwrap(),
            None
        );
        assert!(parse_credentials("not json").is_err());
    }

    #[test]
    fn reads_usage() {
        let body = r#"{"five_hour":{"utilization":42.0,"resets_at":"2025-09-10T17:34:56.123456+00:00"},"seven_day":{"utilization":67,"resets_at":"2025-09-13T12:34:56+00:00"},"seven_day_oauth_apps":null,"seven_day_opus":{"utilization":12.5,"resets_at":null},"seven_day_sonnet":null,"extra_usage":{"is_enabled":false}}"#;
        let s = parse_usage(body, Some("max"), at(T)).unwrap();
        assert_eq!(s.provider, Provider::Claude);
        assert_eq!(s.plan.as_deref(), Some("Max"));
        assert_eq!(s.observed_at, at(T));
        assert_eq!(s.windows.len(), 3);
        let session = s.session().unwrap();
        assert_eq!(session.used, 42.0);
        assert_eq!(
            session.resets_at,
            Some(at(T + 5 * 3600) + Duration::from_micros(123_456))
        );
        assert_eq!(s.week().unwrap().used, 67.0);
        assert_eq!(s.week().unwrap().resets_at, Some(at(T + 3 * 86_400)));
        assert_eq!(s.windows[2].label(), "Week Opus");
        assert_eq!(s.windows[2].resets_at, None);
    }

    #[test]
    fn bad_usage_is_an_error() {
        for body in ["", "[]", "{}", r#"{"five_hour":null}"#] {
            assert!(parse_usage(body, None, at(T)).is_err(), "{body}");
        }
    }

    #[test]
    fn plan_names() {
        assert_eq!(plan_name("max"), "Max");
        assert_eq!(plan_name("pro"), "Pro");
        assert_eq!(plan_name(""), "");
    }
}
