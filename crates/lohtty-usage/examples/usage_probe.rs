//! Prints what the title-bar usage meter would show, once per provider.
//!
//!     cargo run -p lohtty-usage --example usage_probe
//!
//! The Claude token is never printed.

use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use lohtty_usage::{Poller, PollerOptions, ProviderState, humanize_ago, humanize_remaining};

fn main() {
    let (tx, rx) = mpsc::channel();
    let _poller = Poller::spawn(PollerOptions::default(), move |update| {
        let _ = tx.send(update);
    })
    .expect("spawn the poller");

    for _ in 0..2 {
        let Ok(update) = rx.recv_timeout(Duration::from_secs(30)) else {
            println!("(no more updates)");
            break;
        };
        let now = SystemTime::now();
        let name = update.provider.name();
        match update.state {
            ProviderState::Unavailable => println!("{name}: not set up here"),
            ProviderState::Failed(e) => println!("{name}: error: {e}"),
            ProviderState::Ready(s) => {
                let plan = s
                    .plan
                    .as_deref()
                    .map(|p| format!(" · {p}"))
                    .unwrap_or_default();
                let ago = now.duration_since(s.observed_at).unwrap_or_default();
                println!("{name}{plan} (updated {})", humanize_ago(ago));
                for w in &s.windows {
                    let resets = match w.remaining(now) {
                        Some(left) => format!("resets in {}", humanize_remaining(left)),
                        None if w.has_reset(now) => "reset since".to_owned(),
                        None => "reset time unknown".to_owned(),
                    };
                    println!("  {:<12} {:>5.1}%  {resets}", w.label(), w.used_at(now));
                }
            }
        }
    }
}
