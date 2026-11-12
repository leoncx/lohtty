//! A background thread that polls the providers and reports their state.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::{Provider, UsageError, UsageSnapshot, claude, codex};

/// The longest wait after repeated failures.
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// Timeout of a whole request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// What to poll, and how often.
#[derive(Debug, Clone, PartialEq)]
pub struct PollerOptions {
    pub claude: bool,
    pub codex: bool,
    /// Between two requests to Claude's usage endpoint.
    pub claude_interval: Duration,
    /// Between two reads of Codex's logs (local files only).
    pub codex_interval: Duration,
}

impl Default for PollerOptions {
    fn default() -> Self {
        PollerOptions {
            claude: true,
            codex: true,
            claude_interval: Duration::from_secs(120),
            codex_interval: Duration::from_secs(30),
        }
    }
}

/// A provider's state, as last polled.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderState {
    /// The agent isn't set up on this machine: nothing to show.
    Unavailable,
    Ready(UsageSnapshot),
    /// The last poll failed; keep showing the previous snapshot, if any.
    Failed(UsageError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub provider: Provider,
    pub state: ProviderState,
}

enum Command {
    Refresh,
    Stop,
}

/// Polls in the background until dropped. Updates go to the callback, on
/// the poller's thread. Codex updates are sent only when they change.
pub struct Poller {
    commands: mpsc::Sender<Command>,
}

impl Poller {
    pub fn spawn(
        options: PollerOptions,
        on_update: impl FnMut(Update) + Send + 'static,
    ) -> std::io::Result<Poller> {
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("lohtty-usage".to_owned())
            .spawn(move || run(options, receiver, on_update))?;
        Ok(Poller { commands })
    }

    /// Polls now, except a provider that asked to wait (rate limit).
    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }
}

impl Drop for Poller {
    /// Stops the thread. It isn't joined: a request in flight could take up
    /// to the timeout, and nothing waits for its result.
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

/// When a provider is next polled, and whether a refresh may move it.
struct Schedule {
    next: Instant,
    /// Set while backing off after a rate limit: refreshes don't apply.
    hold: bool,
    failures: u32,
}

impl Schedule {
    fn new(now: Instant) -> Schedule {
        Schedule {
            next: now,
            hold: false,
            failures: 0,
        }
    }
}

fn run(
    options: PollerOptions,
    commands: mpsc::Receiver<Command>,
    mut on_update: impl FnMut(Update),
) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let start = Instant::now();
    let mut claude_at = Schedule::new(start);
    let mut codex_at = Schedule::new(start);
    let mut last_codex: Option<ProviderState> = None;

    loop {
        let now = Instant::now();
        if options.claude && now >= claude_at.next {
            let result = poll_claude(&agent);
            let delay = next_delay(&result, &mut claude_at.failures, options.claude_interval);
            claude_at.hold = matches!(result, Err(UsageError::RateLimited { .. }));
            claude_at.next = now + delay;
            on_update(Update {
                provider: Provider::Claude,
                state: into_state(result),
            });
        }
        if options.codex && now >= codex_at.next {
            let state = poll_codex();
            codex_at.next = now + options.codex_interval;
            if last_codex.as_ref() != Some(&state) {
                last_codex = Some(state.clone());
                on_update(Update {
                    provider: Provider::Codex,
                    state,
                });
            }
        }

        let next = [
            options.claude.then_some(claude_at.next),
            options.codex.then_some(codex_at.next),
        ]
        .into_iter()
        .flatten()
        .min();
        let command = match next {
            Some(next) => commands.recv_timeout(next.saturating_duration_since(Instant::now())),
            None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match command {
            Ok(Command::Refresh) => {
                let now = Instant::now();
                for schedule in [&mut claude_at, &mut codex_at] {
                    if !schedule.hold {
                        schedule.next = now;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// `Ok(None)`: Claude Code isn't signed in with a subscription here. The
/// credentials are read each time, so a token Claude Code renewed is used.
fn poll_claude(agent: &ureq::Agent) -> Result<Option<UsageSnapshot>, UsageError> {
    let Some(credentials) = claude::load_credentials()? else {
        return Ok(None);
    };
    claude::fetch(agent, &credentials, SystemTime::now()).map(Some)
}

fn poll_codex() -> ProviderState {
    let Some(home) = codex::codex_home() else {
        return ProviderState::Unavailable;
    };
    match codex::latest_snapshot(&home) {
        Ok(Some(snapshot)) => ProviderState::Ready(snapshot),
        Ok(None) => ProviderState::Unavailable,
        Err(e) => ProviderState::Failed(UsageError::Io(e.to_string())),
    }
}

fn into_state(result: Result<Option<UsageSnapshot>, UsageError>) -> ProviderState {
    match result {
        Ok(Some(snapshot)) => ProviderState::Ready(snapshot),
        Ok(None) => ProviderState::Unavailable,
        Err(e) => ProviderState::Failed(e),
    }
}

/// The wait before the next request. Success, or a state only Claude Code
/// can change (absent or expired sign-in), waits the interval. Server and
/// network failures back off, doubling up to [`MAX_BACKOFF`]; a rate limit
/// waits at least what the server asked.
fn next_delay(
    result: &Result<Option<UsageSnapshot>, UsageError>,
    failures: &mut u32,
    interval: Duration,
) -> Duration {
    let backoff = |failures: &mut u32| {
        *failures = failures.saturating_add(1);
        let factor = 1u32 << (*failures).min(10);
        interval
            .saturating_mul(factor)
            .min(MAX_BACKOFF)
            .max(interval)
    };
    match result {
        Ok(_) | Err(UsageError::Expired | UsageError::Credentials(_)) => {
            *failures = 0;
            interval
        }
        Err(UsageError::RateLimited { retry_after }) => {
            let wait = backoff(failures);
            retry_after.map_or(wait, |asked| asked.max(wait))
        }
        Err(_) => backoff(failures),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(120);

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot {
            provider: Provider::Claude,
            plan: None,
            windows: vec![],
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn success_waits_the_interval_and_resets_failures() {
        let mut failures = 3;
        assert_eq!(
            next_delay(&Ok(Some(snapshot())), &mut failures, INTERVAL),
            INTERVAL
        );
        assert_eq!(failures, 0);
        assert_eq!(next_delay(&Ok(None), &mut failures, INTERVAL), INTERVAL);
        assert_eq!(
            next_delay(&Err(UsageError::Expired), &mut failures, INTERVAL),
            INTERVAL
        );
    }

    #[test]
    fn failures_back_off_up_to_a_cap() {
        let mut failures = 0;
        let err = Err(UsageError::Status(500));
        let delays: Vec<u64> = (0..6)
            .map(|_| next_delay(&err, &mut failures, INTERVAL).as_secs())
            .collect();
        assert_eq!(delays, [240, 480, 960, 1800, 1800, 1800]);
    }

    #[test]
    fn rate_limits_wait_what_the_server_asks() {
        let mut failures = 0;
        let asked = Err(UsageError::RateLimited {
            retry_after: Some(Duration::from_secs(3600)),
        });
        assert_eq!(next_delay(&asked, &mut failures, INTERVAL).as_secs(), 3600);
        let short = Err(UsageError::RateLimited {
            retry_after: Some(Duration::from_secs(1)),
        });
        assert_eq!(next_delay(&short, &mut failures, INTERVAL).as_secs(), 480);
        let unsaid = Err(UsageError::RateLimited { retry_after: None });
        assert_eq!(next_delay(&unsaid, &mut failures, INTERVAL).as_secs(), 960);
    }

    #[test]
    fn a_poller_without_providers_stops_when_dropped() {
        let options = PollerOptions {
            claude: false,
            codex: false,
            ..PollerOptions::default()
        };
        let (tx, rx) = mpsc::channel();
        let poller = Poller::spawn(options, move |u| tx.send(u).unwrap()).unwrap();
        poller.refresh();
        drop(poller);
        // The callback (and its sender) is dropped with the thread.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
