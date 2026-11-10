//! Headless throughput: runs a command in a PTY session (no rendering) and
//! reports how long until it exits.
//!
//! ```text
//! cargo run --release -p lohtty-term --example throughput -- cmd /c type big.txt
//! ```

use std::time::Instant;

use lohtty_term::{PassThrough, Session, SessionEvent, ShellSpec, TermSize};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let program = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: throughput <program> [args...]"))?;
    let shell = ShellSpec::new(program, args.collect());
    let size = TermSize {
        cols: 120,
        rows: 40,
        cell_width: 8,
        cell_height: 16,
    };

    let start = Instant::now();
    let (_session, events) = Session::spawn(&shell, size, 10_000, Box::new(PassThrough))?;
    while let Ok(event) = events.recv_blocking() {
        if matches!(event, SessionEvent::Exit) {
            break;
        }
    }
    println!("{:.2} s", start.elapsed().as_secs_f64());
    Ok(())
}
