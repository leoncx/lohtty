use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::{Processor, Rgb};
use anyhow::Context as _;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{OutputFilter, ShellSpec};

/// Grid size in cells plus the cell size in pixels (needed by some escape sequences).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TermSize {
    fn pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cols.saturating_mul(self.cell_width),
            pixel_height: self.rows.saturating_mul(self.cell_height),
        }
    }

    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// Events from a session, consumed by the UI thread.
pub enum SessionEvent {
    /// New content is available; repaint. Coalesced: at most one is pending at a time.
    Wakeup,
    Title(String),
    ResetTitle,
    Bell,
    ClipboardStore(String),
    /// The application asked for a palette color (OSC 4/10/11/12). The UI resolves
    /// it against the theme and writes `formatter(rgb)` back with [`Session::write`].
    ColorRequest(usize, Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>),
    /// The shell exited or the PTY closed.
    Exit,
}

/// Bridges alacritty's events to the UI channel and the PTY writer.
#[derive(Clone)]
pub struct EventProxy {
    events: async_channel::Sender<SessionEvent>,
    pty_tx: mpsc::Sender<Vec<u8>>,
    size: Arc<Mutex<TermSize>>,
    dirty: Arc<AtomicBool>,
}

impl EventProxy {
    fn wakeup(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            let _ = self.events.try_send(SessionEvent::Wakeup);
        }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let ev = match event {
            Event::Wakeup => return self.wakeup(),
            Event::Title(t) => SessionEvent::Title(t),
            Event::ResetTitle => SessionEvent::ResetTitle,
            Event::Bell => SessionEvent::Bell,
            Event::ClipboardStore(_, s) => SessionEvent::ClipboardStore(s),
            Event::ColorRequest(i, f) => SessionEvent::ColorRequest(i, f),
            Event::PtyWrite(s) => {
                let _ = self.pty_tx.send(s.into_bytes());
                return;
            }
            Event::TextAreaSizeRequest(f) => {
                let size = *self.size.lock().unwrap();
                let _ = self.pty_tx.send(f(size.window_size()).into_bytes());
                return;
            }
            Event::Exit | Event::ChildExit(_) => SessionEvent::Exit,
            // Clipboard reads (OSC 52 query) are not supported yet.
            Event::ClipboardLoad(..) | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                return;
            }
        };
        let _ = self.events.try_send(ev);
    }
}

/// A shell running in a PTY, with its terminal state.
///
/// Threads: a reader (PTY → filter → parser), a writer (input → PTY, so
/// replies never block the parser) and a waiter (child exit).
pub struct Session {
    term: Arc<FairMutex<Term<EventProxy>>>,
    pty_tx: mpsc::Sender<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    size: Arc<Mutex<TermSize>>,
    dirty: Arc<AtomicBool>,
}

impl Session {
    pub fn spawn(
        shell: &ShellSpec,
        size: TermSize,
        scrollback: usize,
        mut filter: Box<dyn OutputFilter>,
    ) -> anyhow::Result<(Self, async_channel::Receiver<SessionEvent>)> {
        let pair = native_pty_system()
            .openpty(size.pty_size())
            .context("failed to open PTY")?;

        let mut cmd = CommandBuilder::new(&shell.program);
        cmd.args(&shell.args);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "lohtty");
        if let Some(home) = dirs::home_dir() {
            cmd.cwd(home);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn `{}`", shell.program))?;
        drop(pair.slave);
        tracing::info!(program = %shell.program, pid = ?child.process_id(), "shell spawned");

        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let mut writer = master.take_writer()?;
        let killer = child.clone_killer();

        let (events_tx, events_rx) = async_channel::unbounded();
        let (pty_tx, pty_rx) = mpsc::channel::<Vec<u8>>();
        let size_cell = Arc::new(Mutex::new(size));
        let dirty = Arc::new(AtomicBool::new(false));

        let proxy = EventProxy {
            events: events_tx.clone(),
            pty_tx: pty_tx.clone(),
            size: size_cell.clone(),
            dirty: dirty.clone(),
        };
        let config = TermConfig {
            scrolling_history: scrollback,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, proxy.clone())));

        thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || {
                for bytes in pty_rx {
                    if writer
                        .write_all(&bytes)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;

        let reader_term = term.clone();
        let reader_pty_tx = pty_tx.clone();
        thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut processor: Processor = Processor::new();
                let mut buf = vec![0u8; 64 * 1024];
                let (mut out, mut reply) = (Vec::new(), Vec::new());
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            tracing::debug!("pty read ended: {e}");
                            break;
                        }
                    };
                    tracing::trace!(target: "lohtty_term::pty", "{:?}", String::from_utf8_lossy(&buf[..n]));
                    out.clear();
                    reply.clear();
                    filter.filter(&buf[..n], &mut out, &mut reply);
                    if !reply.is_empty() {
                        let _ = reader_pty_tx.send(reply.clone());
                    }
                    // TODO: honor the synchronized-update timeout (mode 2026) when
                    // an app never closes its update.
                    processor.advance(&mut *reader_term.lock(), &out);
                    proxy.wakeup();
                }
                let _ = proxy.events.try_send(SessionEvent::Exit);
            })?;

        thread::Builder::new()
            .name("pty-child".into())
            .spawn(move || {
                let status = child.wait();
                tracing::info!(?status, "shell exited");
                let _ = events_tx.try_send(SessionEvent::Exit);
            })?;

        Ok((
            Self {
                term,
                pty_tx,
                master,
                killer,
                size: size_cell,
                dirty,
            },
            events_rx,
        ))
    }

    pub fn term(&self) -> &Arc<FairMutex<Term<EventProxy>>> {
        &self.term
    }

    /// Sends input bytes to the shell.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.pty_tx.send(bytes.into());
    }

    pub fn size(&self) -> TermSize {
        *self.size.lock().unwrap()
    }

    /// Resizes the grid and the PTY. No-op if the size is unchanged.
    pub fn resize(&self, new: TermSize) {
        {
            let mut size = self.size.lock().unwrap();
            if *size == new {
                return;
            }
            *size = new;
        }
        if let Err(e) = self.master.resize(new.pty_size()) {
            tracing::warn!("PTY resize failed: {e}");
        }
        self.term.lock().resize(new);
    }

    /// Scrolls the viewport through the scrollback (positive = up).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Clears the pending-wakeup flag. Call before reading the grid for a repaint.
    pub fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Release);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}
