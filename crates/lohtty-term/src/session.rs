use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};
use anyhow::Context as _;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::modes::ModeWatcher;
use crate::notify::{Notification, OscEvent, OscScanner};
use crate::record::{Options as RecordOptions, Recorder};
use crate::status::{QUERY_REPLY, Shown, StatusBoard};
use crate::{OutputFilter, ShellSpec};

/// How long the size must stay unchanged before the PTY gets it. A window
/// animating (zoom, full screen) changes size every frame, and a shell redraws
/// its prompt on each change, leaving copies behind; it gets the last one only.
const RESIZE_SETTLE: Duration = Duration::from_millis(100);

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
    /// A program asked for a desktop notification (OSC 9, 777 or 99).
    Notification(Notification),
    /// The pane's program status records changed (OSC 7501); read them with
    /// [`Session::status`].
    StatusChanged,
    /// A recording ended because the shell exited; the file is complete.
    RecordingSaved(PathBuf),
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
                let _ = self.pty_tx.send(device_attributes(s).into_bytes());
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

/// alacritty_terminal answers Primary Device Attributes (`CSI c`) with
/// `CSI ? 6 c` (a VT102). kitty's own client (`kitty icat`) ignores a reply
/// that short and waits forever, so report a VT220 with ANSI color
/// (`CSI ? 62 ; 22 c`), a form clients generally expect.
fn device_attributes(reply: String) -> String {
    if reply == "\x1b[?6c" {
        "\x1b[?62;22c".to_owned()
    } else {
        reply
    }
}

/// A shell running in a PTY, with its terminal state.
///
/// Threads: a reader (PTY → filter → parser), a writer (input → PTY, so
/// replies never block the parser), a resizer (sizes → PTY, see
/// [`RESIZE_SETTLE`]) and a waiter (child exit).
pub struct Session {
    term: Arc<FairMutex<Term<EventProxy>>>,
    pty_tx: mpsc::Sender<Vec<u8>>,
    resize_tx: mpsc::Sender<TermSize>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    size: Arc<Mutex<TermSize>>,
    dirty: Arc<AtomicBool>,
    program: String,
    /// Whether ConPTY asked for win32 input mode (see [`crate::modes`]).
    win32_input: Arc<AtomicBool>,
    /// Program status records (OSC 7501).
    status: Arc<Mutex<StatusBoard>>,
    /// Reads the shell's working directory and whether it runs a program.
    probe: ShellProbe,
    /// The asciicast recording in progress, if any (see [`crate::record`]).
    recorder: Arc<Mutex<Option<Recorder>>>,
}

/// Looks at the shell from any thread: its working directory, and whether it
/// is at its prompt.
#[derive(Clone)]
pub struct ShellProbe {
    /// The shell's process id, and the working directory it last reported.
    pid: Option<u32>,
    reported_cwd: Arc<Mutex<Option<PathBuf>>>,
    /// The PTY master, to ask for the foreground process group. Owned by
    /// the `pty-resize` thread, which lives as long as the session.
    #[cfg(unix)]
    master_fd: Option<std::os::fd::RawFd>,
}

impl ShellProbe {
    /// The shell's working directory (see [`crate::cwd`]). `None` when
    /// neither the shell reported it nor the platform can read it.
    pub fn cwd(&self) -> Option<PathBuf> {
        let reported = self.reported_cwd.lock().unwrap().clone();
        let process = self.pid.and_then(crate::cwd::process_cwd);
        crate::cwd::pick(reported.as_deref(), process.as_deref())
    }

    /// Whether the shell is in the foreground, not running a program.
    /// `None` when it can't be told.
    pub fn shell_in_foreground(&self) -> Option<bool> {
        let pid = self.pid?;
        #[cfg(unix)]
        return crate::foreground::shell_in_foreground(pid, self.master_fd?);
        #[cfg(windows)]
        return crate::foreground::shell_in_foreground(pid);
        #[cfg(not(any(unix, windows)))]
        {
            let _ = pid;
            None
        }
    }

    /// The shell's working directory while the shell is in the foreground;
    /// `None` while a program runs, or when either can't be told.
    pub fn shell_dir(&self) -> Option<PathBuf> {
        match self.shell_in_foreground() {
            Some(true) => self.cwd(),
            _ => None,
        }
    }
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

        let mut cmd = if shell.login {
            // portable-pty picks the same shell as `shell.program`.
            CommandBuilder::new_default_prog()
        } else {
            let mut cmd = CommandBuilder::new(&shell.program);
            cmd.args(&shell.args);
            cmd
        };
        // Apps opened from Finder have no locale set, so shells would fall
        // back to ASCII. Terminal.app sets the same.
        if cfg!(target_os = "macos")
            && ["LC_ALL", "LC_CTYPE", "LANG"]
                .iter()
                .all(|v| std::env::var_os(v).is_none())
        {
            cmd.env("LC_CTYPE", "UTF-8");
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "lohtty");
        if let Some(dir) = shell
            .cwd
            .clone()
            .filter(|d| d.is_dir())
            .or_else(dirs::home_dir)
        {
            cmd.cwd(dir);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn `{}`", shell.program))?;
        drop(pair.slave);
        let pid = child.process_id();
        tracing::info!(program = %shell.program, ?pid, "shell spawned");

        let master = pair.master;
        #[cfg(unix)]
        let master_fd = master.as_raw_fd();
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
            // Programs may ask for the kitty keyboard protocol (Shift+Enter
            // and other modified keys reported unambiguously).
            kitty_keyboard: true,
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

        // Synchronized updates (mode 2026) buffer output until the app ends
        // them. The reader blocks in `read`, so a watchdog enforces the
        // parser's deadline when an app never ends its update (e.g. it crashed).
        let processor = Arc::new(Mutex::new(Processor::<StdSyncHandler>::new()));
        let reader_done = Arc::new(AtomicBool::new(false));
        let watchdog = {
            let (processor, term, proxy, done) = (
                processor.clone(),
                term.clone(),
                proxy.clone(),
                reader_done.clone(),
            );
            thread::Builder::new()
                .name("pty-sync".into())
                .spawn(move || sync_watchdog(&processor, &term, &proxy, &done))?
        };

        let win32_input = Arc::new(AtomicBool::new(false));
        let reader_win32_input = win32_input.clone();
        let status = Arc::new(Mutex::new(StatusBoard::default()));
        let reader_status = status.clone();
        let reported_cwd = Arc::new(Mutex::new(None));
        let reader_cwd = reported_cwd.clone();
        let reader_term = term.clone();
        let reader_pty_tx = pty_tx.clone();
        let recorder: Arc<Mutex<Option<Recorder>>> = Arc::default();
        let reader_recorder = recorder.clone();
        let child_recorder = recorder.clone();
        thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                let (mut out, mut reply) = (Vec::new(), Vec::new());
                let mut notifications = OscScanner::default();
                let mut modes = ModeWatcher::default();
                let mut found = Vec::new();
                let mut marker = crate::prompt::PromptMarker::new();
                let mut marked = Vec::new();
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
                    notifications.scan(&buf[..n], &mut found);
                    if let Some(on) = modes.scan(&buf[..n]) {
                        reader_win32_input.store(on, Ordering::Relaxed);
                    }
                    out.clear();
                    reply.clear();
                    let mut status_changed = false;
                    for event in found.drain(..) {
                        let mut board = reader_status.lock().unwrap();
                        status_changed |= match event {
                            OscEvent::Notification(note) => {
                                let _ = proxy.events.try_send(SessionEvent::Notification(note));
                                false
                            }
                            // Answered before anything is parsed, so the
                            // reply comes ahead of the DA1 that follows it.
                            OscEvent::Status(crate::status::Message::Query) => {
                                reply.extend_from_slice(QUERY_REPLY);
                                false
                            }
                            OscEvent::Status(message) => board.apply(message),
                            OscEvent::Progress(progress) => board.progress(progress),
                            OscEvent::PromptStart => {
                                // Prompts are the recording's chapters.
                                if let Some(rec) = reader_recorder.lock().unwrap().as_ref() {
                                    rec.marker("");
                                }
                                board.prompt()
                            }
                            OscEvent::Reset => board.reset(),
                            OscEvent::WorkingDir(dir) => {
                                *reader_cwd.lock().unwrap() = Some(dir);
                                false
                            }
                        };
                    }
                    if status_changed {
                        let _ = proxy.events.try_send(SessionEvent::StatusChanged);
                    }
                    // Prompt marks first: the kitty filter's checkpoints are
                    // offsets into its own output.
                    marked.clear();
                    marker.mark(&buf[..n], &mut marked);
                    filter.filter(&marked, &mut out, &mut reply);
                    // Filter replies go first, before the parser's own (a
                    // client may wait for its graphics replies, then DA1).
                    if !reply.is_empty() {
                        let _ = reader_pty_tx.send(reply.clone());
                    }
                    let checkpoints = filter.take_checkpoints();
                    let syncing = {
                        let mut processor = processor.lock().unwrap();
                        let mut term = reader_term.lock();
                        // Recorded under the terminal lock, so a recording
                        // starting now has these bytes either in its screen
                        // snapshot or as events, never both or neither.
                        if let Some(rec) = reader_recorder.lock().unwrap().as_mut() {
                            rec.output(&buf[..n]);
                        }
                        // Parse up to each point where the filter needs the
                        // screen, and show it the screen there.
                        let mut start = 0;
                        for at in checkpoints {
                            let at = at.clamp(start, out.len());
                            processor.advance(&mut *term, &out[start..at]);
                            filter.at_checkpoint(&*term);
                            start = at;
                        }
                        processor.advance(&mut *term, &out[start..]);
                        processor.sync_timeout().sync_timeout().is_some()
                    };
                    if syncing {
                        watchdog.thread().unpark();
                    }
                    proxy.wakeup();
                }
                reader_done.store(true, Ordering::Release);
                watchdog.thread().unpark();
                let _ = proxy.events.try_send(SessionEvent::Exit);
            })?;

        let (resize_tx, resize_rx) = mpsc::channel();
        thread::Builder::new()
            .name("pty-resize".into())
            .spawn(move || resize_pty(&*master, &resize_rx))?;

        thread::Builder::new()
            .name("pty-child".into())
            .spawn(move || {
                let status = child.wait();
                tracing::info!(?status, "shell exited");
                if let Some(rec) = child_recorder.lock().unwrap().take() {
                    let code = status.as_ref().map(|s| s.exit_code() as i32).ok();
                    let path = rec.finish(code);
                    let _ = events_tx.try_send(SessionEvent::RecordingSaved(path));
                }
                let _ = events_tx.try_send(SessionEvent::Exit);
            })?;

        Ok((
            Self {
                term,
                pty_tx,
                resize_tx,
                killer,
                size: size_cell,
                dirty,
                program: shell.program.clone(),
                win32_input,
                status,
                probe: ShellProbe {
                    pid,
                    reported_cwd,
                    #[cfg(unix)]
                    master_fd,
                },
                recorder,
            },
            events_rx,
        ))
    }

    /// The shell's working directory, to start a new pane in (see
    /// [`ShellProbe::cwd`]).
    pub fn cwd(&self) -> Option<PathBuf> {
        self.probe.cwd()
    }

    /// Looks at the shell from another thread.
    pub fn probe(&self) -> ShellProbe {
        self.probe.clone()
    }

    /// The program running in this session, as given to [`Session::spawn`].
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The program status records to show, most urgent first.
    pub fn status(&self) -> Vec<Shown> {
        self.status.lock().unwrap().shown()
    }

    /// The user saw the pane: finished results stop showing. Returns
    /// whether anything changed.
    pub fn status_seen(&self) -> bool {
        self.status.lock().unwrap().seen()
    }

    /// Whether ConPTY asked for win32 input mode: keys can then be sent with
    /// [`crate::input::encode_win32`].
    pub fn win32_input_mode(&self) -> bool {
        self.win32_input.load(Ordering::Relaxed)
    }

    pub fn term(&self) -> &Arc<FairMutex<Term<EventProxy>>> {
        &self.term
    }

    /// Sends input bytes to the shell.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
            rec.input(&bytes);
        }
        let _ = self.pty_tx.send(bytes);
    }

    /// Starts recording this pane to `path` (asciicast), from the screen as
    /// it is now.
    pub fn start_recording(&self, path: &Path, options: &RecordOptions) -> std::io::Result<()> {
        // Terminal lock first, as the reader takes them (see the reader).
        let term = self.term.lock();
        let mut recorder = self.recorder.lock().unwrap();
        if recorder.is_some() {
            return Err(std::io::Error::other("already recording"));
        }
        let size = *self.size.lock().unwrap();
        let snapshot = crate::record::screen_snapshot(&*term);
        *recorder = Some(Recorder::start(
            path, options, size.cols, size.rows, &snapshot,
        )?);
        Ok(())
    }

    /// Stops the recording, if any, and returns its file once written.
    pub fn stop_recording(&self) -> Option<PathBuf> {
        let recorder = self.recorder.lock().unwrap().take();
        recorder.map(|rec| rec.finish(None))
    }

    /// The file being recorded to, if any.
    pub fn recording(&self) -> Option<PathBuf> {
        self.recorder
            .lock()
            .unwrap()
            .as_ref()
            .map(|rec| rec.path().to_owned())
    }

    pub fn size(&self) -> TermSize {
        *self.size.lock().unwrap()
    }

    /// Resizes the grid now and the PTY once the size settles (see
    /// [`RESIZE_SETTLE`]). No-op if the size is unchanged.
    pub fn resize(&self, new: TermSize) {
        {
            let mut size = self.size.lock().unwrap();
            if *size == new {
                return;
            }
            *size = new;
        }
        if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
            rec.resize(new.cols, new.rows);
        }
        let _ = self.resize_tx.send(new);
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

/// Passes sizes to the PTY until the session is dropped. The first one goes
/// at once (the shell starts at a guessed size); later ones once no newer
/// size has come for [`RESIZE_SETTLE`].
fn resize_pty(master: &dyn MasterPty, sizes: &mpsc::Receiver<TermSize>) {
    let apply = |size: TermSize| {
        if let Err(e) = master.resize(size.pty_size()) {
            tracing::warn!("PTY resize failed: {e}");
        }
    };
    let Ok(first) = sizes.recv() else { return };
    apply(first);
    while let Ok(mut size) = sizes.recv() {
        loop {
            match sizes.recv_timeout(RESIZE_SETTLE) {
                Ok(newer) => size = newer,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        apply(size);
    }
}

/// Sleeps until woken by the reader, then flushes any synchronized update
/// that outlives its deadline.
fn sync_watchdog(
    processor: &Mutex<Processor<StdSyncHandler>>,
    term: &FairMutex<Term<EventProxy>>,
    proxy: &EventProxy,
    done: &AtomicBool,
) {
    while !done.load(Ordering::Acquire) {
        thread::park();
        loop {
            let deadline = processor.lock().unwrap().sync_timeout().sync_timeout();
            let Some(deadline) = deadline else { break };
            let now = Instant::now();
            if now < deadline {
                thread::sleep(deadline - now);
                continue;
            }
            let mut processor = processor.lock().unwrap();
            // Re-check under the lock: the update may have ended meanwhile.
            if processor
                .sync_timeout()
                .sync_timeout()
                .is_some_and(|d| Instant::now() >= d)
            {
                tracing::debug!("synchronized update timed out; flushing");
                processor.stop_sync(&mut *term.lock());
                drop(processor);
                proxy.wakeup();
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::device_attributes;

    #[test]
    fn primary_device_attributes_are_long_enough_for_kitty_clients() {
        // kitty icat ignores a DA1 reply of three characters or fewer.
        assert_eq!(device_attributes("\x1b[?6c".into()), "\x1b[?62;22c");
        // Other replies pass unchanged.
        assert_eq!(device_attributes("\x1b[>0;0;1c".into()), "\x1b[>0;0;1c");
    }
}
