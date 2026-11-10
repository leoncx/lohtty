//! Spike: does an APC sequence (kitty graphics) survive ConPTY?
//! Runs a command that prints one, and dumps every ESC sequence the PTY
//! delivers that isn't plain CSI/OSC.
//!
//! ```text
//! cargo run -p lohtty-term --example apc_probe
//! ```

use std::sync::{Arc, Mutex};

use lohtty_term::{OutputFilter, Session, SessionEvent, ShellSpec, TermSize};

struct Probe(Arc<Mutex<Vec<u8>>>);

impl OutputFilter for Probe {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, _reply: &mut Vec<u8>) {
        self.0.lock().unwrap().extend_from_slice(input);
        out.extend_from_slice(input);
    }
}

fn main() -> anyhow::Result<()> {
    let shell = ShellSpec::new("pwsh.exe", vec![
            "-NoProfile".into(),
            "-Command".into(),
            r#"[Console]::Out.Write("before`e_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA`e\after"); [Console]::Out.Flush(); Start-Sleep -Milliseconds 300"#.into(),
        ]);
    let size = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (_session, events) = Session::spawn(&shell, size, 1000, Box::new(Probe(seen.clone())))?;
    while let Ok(event) = events.recv_blocking() {
        if matches!(event, SessionEvent::Exit) {
            break;
        }
    }
    let bytes = seen.lock().unwrap();
    let text = String::from_utf8_lossy(&bytes);
    println!(
        "{} bytes; APC present: {}",
        bytes.len(),
        text.contains("\x1b_G")
    );
    println!("{}", text.escape_debug());
    Ok(())
}
