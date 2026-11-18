//! The terminal grid view: paints a [`Session`]'s screen and forwards input.

mod box_drawing;
#[cfg(feature = "kitty-graphics")]
mod images;
mod input;
mod paint;
mod prompts;
mod search;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, ElementInputHandler,
    EventEmitter, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight, IntoElement,
    KeyBinding, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Subscription, Task, Window,
    actions, canvas, div, prelude::*, px,
};
use lohtty_config::{Config, FontConfig};
use lohtty_term::notify::Notification;
use lohtty_term::status::{Kind, Record, Shown, State};
use lohtty_term::{PassThrough, Session, SessionEvent, TermSize, resolve_shell};
use lohtty_theme::Theme;

use crate::colors;
use crate::fonts;
use crate::paint_now::PaintNow;
use paint::FramePainter;

actions!(
    terminal,
    [
        Copy,
        Paste,
        Search,
        /// Scrolls to the previous shell prompt (shell integration marks).
        PreviousPrompt,
        /// Scrolls to the next shell prompt.
        NextPrompt,
        /// Selects and copies the last command's output.
        SelectLastOutput,
        /// Makes this pane's text one point larger.
        IncreaseFontSize,
        /// Makes this pane's text one point smaller.
        DecreaseFontSize,
        /// Back to the configured font size.
        ResetFontSize
    ]
);

/// Smallest and largest font sizes reachable by zooming, in points.
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 72.0;

const KEY_CONTEXT: &str = "Terminal";

/// Minimum interval between output-driven repaints while output streams.
/// The repaint itself lands on the next vsync, so 12 ms gives roughly 50-60
/// fps on 60-165 Hz displays: a screen changing faster can't be read anyway.
const MIN_OUTPUT_FRAME: Duration = Duration::from_millis(12);

/// Output arriving this soon after user input is treated as its echo and
/// painted immediately (see [`PaintNow`]); other output waits for vsync.
const ECHO_WINDOW: Duration = Duration::from_millis(250);

/// How long after output the shell's directory is looked at (see
/// [`TerminalView::check_shell_dir`]).
const SHELL_DIR_CHECK_DELAY: Duration = Duration::from_millis(200);

/// Events a terminal view reports to its container (tab / workspace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    TitleChanged,
    /// The view gained keyboard focus (it becomes its tab's active pane).
    Focused,
    /// The shell exited; the pane should close.
    Exited,
    /// A program rang the bell (`None`) or asked for a notification: it
    /// may need the user (an agent finished or waits for input).
    Attention(Option<Notification>),
    /// The program status shown for this pane changed (OSC 7501).
    StatusChanged,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

/// Default key bindings for terminal views.
pub fn key_bindings() -> Vec<KeyBinding> {
    let ctx = Some(KEY_CONTEXT);
    // Search also works from the search bar itself (re-focuses it).
    let search_ctx = Some("Terminal || SearchBar");
    if cfg!(target_os = "macos") {
        vec![
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("cmd-f", Search, search_ctx),
            KeyBinding::new("cmd-shift-up", PreviousPrompt, ctx),
            KeyBinding::new("cmd-shift-down", NextPrompt, ctx),
            KeyBinding::new("cmd-shift-o", SelectLastOutput, ctx),
            KeyBinding::new("cmd-=", IncreaseFontSize, ctx),
            KeyBinding::new("cmd-+", IncreaseFontSize, ctx),
            KeyBinding::new("cmd--", DecreaseFontSize, ctx),
            KeyBinding::new("cmd-0", ResetFontSize, ctx),
        ]
    } else {
        vec![
            KeyBinding::new("ctrl-shift-c", Copy, ctx),
            KeyBinding::new("ctrl-shift-v", Paste, ctx),
            KeyBinding::new("ctrl-insert", Copy, ctx),
            KeyBinding::new("shift-insert", Paste, ctx),
            KeyBinding::new("ctrl-shift-f", Search, search_ctx),
            KeyBinding::new("ctrl-shift-up", PreviousPrompt, ctx),
            KeyBinding::new("ctrl-shift-down", NextPrompt, ctx),
            KeyBinding::new("ctrl-shift-o", SelectLastOutput, ctx),
            KeyBinding::new("ctrl-=", IncreaseFontSize, ctx),
            KeyBinding::new("ctrl-+", IncreaseFontSize, ctx),
            KeyBinding::new("ctrl--", DecreaseFontSize, ctx),
            KeyBinding::new("ctrl-0", ResetFontSize, ctx),
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct CellMetrics {
    font_size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
}

/// Where the grid was last painted, for mapping mouse positions to cells.
#[derive(Debug, Clone, Copy)]
struct GridGeometry {
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    cols: usize,
    rows: usize,
}

impl GridGeometry {
    /// The cell under `pos` (clamped to the grid) and which half of it.
    fn cell_at(&self, pos: Point<Pixels>) -> (usize, usize, Side) {
        let x = (pos.x - self.origin.x) / self.cell_width;
        let y = (pos.y - self.origin.y) / self.line_height;
        let col = (x.max(0.0) as usize).min(self.cols - 1);
        let row = (y.max(0.0) as usize).min(self.rows - 1);
        let side = if x.fract() < 0.5 || x < 0.0 {
            Side::Left
        } else {
            Side::Right
        };
        (col, row, side)
    }

    fn contains(&self, pos: Point<Pixels>) -> bool {
        let x = (pos.x - self.origin.x) / self.cell_width;
        let y = (pos.y - self.origin.y) / self.line_height;
        x >= 0.0 && y >= 0.0 && (x as usize) < self.cols && (y as usize) < self.rows
    }
}

/// Written during paint, read by input handlers.
#[derive(Debug, Clone, Copy, Default)]
struct PaintState {
    grid: Option<GridGeometry>,
    /// The cursor cell, for positioning the IME window.
    cursor: Option<Bounds<Pixels>>,
}

pub struct TerminalView {
    session: Rc<Session>,
    /// Set by the application (OSC 0/2), or the shell's name.
    title: String,
    default_title: String,
    /// The shell's working directory, shown as the title while the shell is
    /// at its prompt (see [`TerminalView::check_shell_dir`]).
    shell_dir: Option<String>,
    shell_dir_check_pending: bool,
    theme: Theme,
    font: FontConfig,
    focus: FocusHandle,
    fonts: Option<FontSet>,
    painted: Rc<Cell<PaintState>>,
    scroll_accum: f32,
    /// IME composition in progress (dead key or IME), if any.
    marked_text: Option<String>,
    /// Key bytes held back until a pending dead key is resolved.
    deferred_input: Option<Vec<u8>>,
    /// A local (non-reported) mouse selection is being dragged.
    selecting: bool,
    /// Button held during an application-reported press, and last reported cell.
    reported_button: Option<lohtty_term::mouse::MouseButton>,
    last_reported_cell: Option<(usize, usize)>,
    /// The open search bar and its state.
    search: Option<search::SearchState>,
    /// Fade the text while neither the grid nor its search bar has focus.
    dim_unfocused: bool,
    /// Points added to the configured font size (Ctrl+= / Ctrl+-).
    font_zoom: f32,
    /// Ctrl+wheel movement not yet turned into a zoom step.
    zoom_accum: f32,
    /// When the last output-driven repaint was requested, and whether one is
    /// already scheduled (see [`TerminalView::output_frame`]).
    last_output_frame: Option<Instant>,
    output_frame_pending: bool,
    /// Repaints immediately (set on each render, once the window is known).
    paint_now: PaintNow,
    /// When input was last sent to the shell.
    last_input: Option<Instant>,
    #[cfg(feature = "kitty-graphics")]
    images: Option<images::ImageLayer>,
    _events: Task<()>,
    _subscriptions: Vec<Subscription>,
    /// The most urgent program status last reported, to notice changes.
    last_status: Option<State>,
}

/// A started shell, ready to be shown by a [`TerminalView`].
pub struct Spawned {
    session: Session,
    events: async_channel::Receiver<SessionEvent>,
    #[cfg(feature = "kitty-graphics")]
    graphics: Option<lohtty_term::kitty::Graphics>,
}

/// Starts the configured shell, in `cwd` if given (else the home directory).
/// Separate from [`TerminalView::new`] so that failures surface before a view
/// is created.
pub fn spawn_session(config: &Config, cwd: Option<PathBuf>) -> anyhow::Result<Spawned> {
    let mut shell = resolve_shell(config.shell.program.as_deref(), &config.shell.args);
    shell.cwd = cwd;
    // The real size is applied on first paint, once the layout is known.
    let initial = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    #[cfg(feature = "kitty-graphics")]
    let graphics = config
        .graphics
        .kitty
        .then(lohtty_term::kitty::Graphics::new);
    #[cfg(feature = "kitty-graphics")]
    let filter: Box<dyn lohtty_term::OutputFilter> = match &graphics {
        Some(g) => Box::new(g.filter()),
        None => Box::new(PassThrough),
    };
    #[cfg(not(feature = "kitty-graphics"))]
    let filter = Box::new(PassThrough);
    let (session, events) = Session::spawn(&shell, initial, config.scrollback, filter)?;
    Ok(Spawned {
        session,
        events,
        #[cfg(feature = "kitty-graphics")]
        graphics,
    })
}

impl TerminalView {
    pub fn new(
        spawned: Spawned,
        config: &Config,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Spawned {
            session,
            events,
            #[cfg(feature = "kitty-graphics")]
            graphics,
        } = spawned;
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive =
                    this.update_in(cx, |this, window, cx| this.handle_event(event, window, cx));
                if alive.is_err() {
                    break;
                }
            }
        });

        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus_in(&focus, window, |this, _, cx| this.focus_changed(true, cx)),
            cx.on_blur(&focus, window, |this, _, cx| this.focus_changed(false, cx)),
        ];

        // "C:\...\pwsh.exe" -> "pwsh"
        let default_title = std::path::Path::new(session.program())
            .file_stem()
            .map_or_else(
                || session.program().to_owned(),
                |s| s.to_string_lossy().into_owned(),
            );

        Self {
            session: Rc::new(session),
            title: default_title.clone(),
            default_title,
            shell_dir: None,
            shell_dir_check_pending: false,
            theme,
            font: config.font.clone(),
            dim_unfocused: config.window.dim_unfocused,
            focus,
            last_status: None,
            fonts: None,
            painted: Rc::default(),
            scroll_accum: 0.0,
            marked_text: None,
            deferred_input: None,
            selecting: false,
            reported_button: None,
            last_reported_cell: None,
            search: None,
            font_zoom: 0.0,
            zoom_accum: 0.0,
            last_output_frame: None,
            output_frame_pending: false,
            paint_now: PaintNow::default(),
            last_input: None,
            #[cfg(feature = "kitty-graphics")]
            images: graphics.map(images::ImageLayer::new),
            _events: task,
            _subscriptions: subscriptions,
        }
    }

    /// Applies a reloaded configuration (theme, font). The shell keeps running.
    pub fn apply_config(&mut self, config: &Config, theme: Theme, cx: &mut Context<Self>) {
        if self.font != config.font {
            self.font = config.font.clone();
            self.fonts = None;
        }
        if let Some(search) = &self.search {
            let theme = theme.clone();
            search.bar.update(cx, |bar, cx| bar.set_theme(theme, cx));
        }
        self.theme = theme;
        self.dim_unfocused = config.window.dim_unfocused;
        cx.notify();
    }

    /// Repaints for new output, at most once per [`MIN_OUTPUT_FRAME`] while
    /// output streams continuously. Isolated output (e.g. a keystroke's echo)
    /// paints immediately, so typing latency is unaffected.
    fn output_frame(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let since_last = self.last_output_frame.map(|t| now - t);
        match since_last {
            Some(elapsed) if elapsed < MIN_OUTPUT_FRAME => {
                if self.output_frame_pending {
                    return;
                }
                self.output_frame_pending = true;
                let wait = MIN_OUTPUT_FRAME - elapsed;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.output_frame_pending = false;
                        this.last_output_frame = Some(Instant::now());
                        cx.notify();
                    });
                })
                .detach();
            }
            _ => {
                self.last_output_frame = Some(now);
                cx.notify();
                // A keystroke's echo: show it without waiting for the next
                // vsync tick. Streaming output keeps the vsync pacing.
                if self.last_input.is_some_and(|t| now - t < ECHO_WINDOW) {
                    self.paint_now.request();
                }
            }
        }
    }

    /// The shell's working directory while it is at its prompt, else the
    /// title set by the running program.
    pub fn title(&self) -> &str {
        self.shell_dir.as_deref().unwrap_or(&self.title)
    }

    /// Looks, shortly after output, whether the shell is at its prompt and
    /// in which directory. Output is what follows a `cd`, a program starting
    /// or a program ending (the next prompt), and the delay lets a program
    /// that was just typed start. At most one check is pending, so streaming
    /// output checks every [`SHELL_DIR_CHECK_DELAY`].
    fn check_shell_dir(&mut self, cx: &mut Context<Self>) {
        if self.shell_dir_check_pending {
            return;
        }
        self.shell_dir_check_pending = true;
        let probe = self.session.probe();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHELL_DIR_CHECK_DELAY).await;
            let dir = cx
                .background_executor()
                .spawn(async move { probe.shell_dir().map(|d| lohtty_term::display_dir(&d)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.shell_dir_check_pending = false;
                if this.shell_dir != dir {
                    this.shell_dir = dir;
                    cx.emit(TerminalEvent::TitleChanged);
                }
            });
        })
        .detach();
    }

    /// The shell's working directory, for a new pane to start in.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.session.cwd()
    }

    fn set_title(&mut self, title: String, cx: &mut Context<Self>) {
        if self.title != title {
            self.title = title;
            cx.emit(TerminalEvent::TitleChanged);
        }
    }

    fn handle_event(&mut self, event: SessionEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Wakeup => {
                crate::perf::output_received();
                self.output_frame(cx);
                self.check_shell_dir(cx);
            }
            SessionEvent::Title(title) => self.set_title(title, cx),
            SessionEvent::ResetTitle => self.set_title(self.default_title.clone(), cx),
            SessionEvent::Bell => cx.emit(TerminalEvent::Attention(None)),
            SessionEvent::Notification(note) => cx.emit(TerminalEvent::Attention(Some(note))),
            SessionEvent::StatusChanged => self.status_changed(cx),
            SessionEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            SessionEvent::ColorRequest(index, format) => {
                let rgb = {
                    let term = self.session.term().lock();
                    colors::palette(index, term.colors(), &self.theme)
                };
                self.session.write(format(colors::to_term(rgb)));
            }
            SessionEvent::Exit => cx.emit(TerminalEvent::Exited),
        }
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self
            .session
            .term()
            .lock()
            .mode()
            .contains(TermMode::FOCUS_IN_OUT)
        {
            self.session
                .write(if focused { &b"\x1b[I"[..] } else { b"\x1b[O" });
        }
        if focused {
            cx.emit(TerminalEvent::Focused);
            self.mark_seen(cx);
        }
        // Repaint: the cursor is hollow while unfocused.
        cx.notify();
    }

    /// The program status records to show, most urgent first.
    pub fn status(&self) -> Vec<Shown> {
        self.session.status()
    }

    /// The user is looking at the pane: finished results stop showing.
    pub fn mark_seen(&mut self, cx: &mut Context<Self>) {
        if self.session.status_seen() {
            self.status_changed(cx);
        }
    }

    /// Reports a status change; a pane that becomes blocked, done or failed
    /// also asks for attention, with the program's message.
    fn status_changed(&mut self, cx: &mut Context<Self>) {
        let top = self.session.status().into_iter().next();
        let state = top.as_ref().map(|s| s.record.state);
        if state != self.last_status
            && let Some(Shown { record, .. }) = &top
            && matches!(record.state, State::Blocked | State::Done | State::Error)
        {
            cx.emit(TerminalEvent::Attention(Some(status_notification(record))));
        }
        self.last_status = state;
        cx.emit(TerminalEvent::StatusChanged);
    }

    /// The fonts and cell metrics for the current config and display scale,
    /// computed once.
    fn font_set(&mut self, window: &mut Window) -> FontSet {
        let scale = window.scale_factor();
        if let Some(set) = self.fonts.as_ref().filter(|set| set.scale == scale) {
            return set.clone();
        }
        let fallbacks = fonts::fallbacks(&self.font, window);
        let family = fonts::family(&self.font.family, window);
        let make = |bold: bool, italic: bool| Font {
            family: family.clone(),
            features: if self.font.ligatures {
                // Explicit, not `default()`: on Windows gpui applies an empty
                // feature list as "no features", which disables ligatures.
                FontFeatures(Arc::new(vec![("calt".into(), 1), ("liga".into(), 1)]))
            } else {
                FontFeatures::disable_ligatures()
            },
            fallbacks: fallbacks.clone(),
            weight: if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        };
        let fonts = [
            make(false, false),
            make(true, false),
            make(false, true),
            make(true, true),
        ];

        let font_size = px(self.font_size());
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&fonts[0]);
        let advance = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        // Cells are whole device pixels, as in other terminals: images that
        // clients cut into cell-sized tiles (yazi) then fit their cells
        // exactly, and every cell edge falls on a pixel.
        let whole_pixels = |v: Pixels| px((f32::from(v) * scale).round().max(1.0) / scale);
        let set = FontSet {
            fonts,
            metrics: CellMetrics {
                font_size,
                cell_width: whole_pixels(advance),
                line_height: whole_pixels(font_size * 1.25),
            },
            scale,
        };
        self.fonts = Some(set.clone());
        set
    }

    /// The configured font size plus this pane's zoom.
    fn font_size(&self) -> f32 {
        (self.font.size + self.font_zoom).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }

    /// Changes this pane's zoom by `points` (0 resets it).
    pub(super) fn zoom_font(&mut self, points: f32, cx: &mut Context<Self>) {
        let before = self.font_size();
        self.font_zoom = if points == 0.0 {
            0.0
        } else {
            // From the size shown, so zooming back from a limit takes one step.
            (before + points).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE) - self.font.size
        };
        if self.font_size() != before {
            self.fonts = None;
            cx.notify();
        }
    }

    fn increase_font_size(&mut self, _: &IncreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_font(1.0, cx);
    }

    fn decrease_font_size(&mut self, _: &DecreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_font(-1.0, cx);
    }

    fn reset_font_size(&mut self, _: &ResetFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_font(0.0, cx);
    }

    fn line_height(&self) -> Pixels {
        self.fonts
            .as_ref()
            .map_or(px(16.0), |f| f.metrics.line_height)
    }
}

/// Regular, bold, italic and bold-italic fonts, plus the cell size they imply.
#[derive(Clone)]
struct FontSet {
    fonts: [Font; 4],
    metrics: CellMetrics,
    /// The display scale the metrics were computed for.
    scale: f32,
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let FontSet { fonts, metrics, .. } = self.font_set(window);
        self.paint_now = PaintNow::for_window(window);
        let painter = FramePainter {
            session: self.session.clone(),
            theme: self.theme.clone(),
            fonts,
            ligatures: self.font.ligatures,
            #[cfg(feature = "kitty-graphics")]
            images: self.images.clone(),
            metrics,
            focused: self.focus.is_focused(window),
            dimmed: self.dim_unfocused
                && !self.focus.is_focused(window)
                && !self
                    .search
                    .as_ref()
                    .is_some_and(|s| s.bar.focus_handle(cx).is_focused(window)),
            painted: self.painted.clone(),
            // A pending dead key isn't shown: it isn't text yet.
            marked_text: self
                .marked_text
                .clone()
                .filter(|_| !self.pending_dead_key()),
            search: self
                .search
                .as_ref()
                .and_then(|s| Some((s.regex.clone()?, s.current.clone()))),
        };
        let view = cx.entity();
        let focus = self.focus.clone();
        #[cfg(feature = "kitty-graphics")]
        let animation_wake = self
            .images
            .as_ref()
            .map(|layer| (layer.animation_wake.clone(), view.downgrade()));

        // The search bar sits outside the "Terminal" key context, so terminal
        // bindings (paste...) and key handling don't apply while typing in it.
        let terminal = div()
            .id("terminal")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .size_full()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::previous_prompt))
            .on_action(cx.listener(Self::next_prompt))
            .on_action(cx.listener(Self::select_last_output))
            .on_action(cx.listener(Self::increase_font_size))
            .on_action(cx.listener(Self::decrease_font_size))
            .on_action(cx.listener(Self::reset_font_size))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_any_mouse_down(cx.listener(Self::on_mouse_down))
            .child(
                canvas(
                    move |bounds, window, _cx| painter.prepaint(bounds, window),
                    move |bounds, frame, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view.clone()),
                            cx,
                        );
                        // Window-level listeners, so drags keep working when the
                        // pointer leaves the grid.
                        let v = view.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                v.update(cx, |this, cx| this.on_mouse_move(ev, cx));
                            }
                        });
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                view.update(cx, |this, cx| this.on_mouse_up(ev, cx));
                            }
                        });
                        #[cfg(feature = "kitty-graphics")]
                        let next_animation_frame = frame.next_animation_frame();
                        frame.paint(window, cx);
                        #[cfg(feature = "kitty-graphics")]
                        if let (Some(at), Some((wake, view))) =
                            (next_animation_frame, animation_wake)
                        {
                            schedule_animation_frame(&wake, at, view, cx);
                        }
                    },
                )
                .size_full(),
            );

        div()
            .relative()
            .size_full()
            .on_action(cx.listener(Self::open_search))
            .child(terminal)
            .children(self.search.as_ref().map(|s| {
                div()
                    .absolute()
                    .top(px(4.0))
                    .right(px(8.0))
                    .child(s.bar.clone())
            }))
    }
}

/// Repaints the view at `at`, when an animated image it shows changes
/// frame, unless a repaint is already due by then.
#[cfg(feature = "kitty-graphics")]
fn schedule_animation_frame(
    wake: &Rc<Cell<Option<Instant>>>,
    at: Instant,
    view: gpui::WeakEntity<TerminalView>,
    cx: &mut App,
) {
    if wake.get().is_some_and(|due| due <= at) {
        return;
    }
    wake.set(Some(at));
    let wake = wake.clone();
    cx.spawn(async move |cx| {
        let wait = at.saturating_duration_since(Instant::now());
        cx.background_executor().timer(wait).await;
        if wake.get() == Some(at) {
            wake.set(None);
        }
        let _ = view.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

/// The notification for a status that needs the user.
fn status_notification(record: &Record) -> Notification {
    let title = record.title.clone().or_else(|| record.app.clone());
    let default = match (record.state, record.kind) {
        (State::Blocked, Some(Kind::Permission)) => "Needs your permission",
        (State::Blocked, Some(Kind::Question)) => "Has a question for you",
        (State::Blocked, Some(Kind::Auth)) => "Needs you to sign in",
        (State::Blocked, None) => "Needs your attention",
        (State::Error, _) => "Failed",
        _ => "Finished",
    };
    Notification {
        title,
        body: record.msg.clone().unwrap_or_else(|| default.to_owned()),
    }
}
