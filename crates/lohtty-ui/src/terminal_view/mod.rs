//! The terminal grid view: paints a [`Session`]'s screen and forwards input.

#[cfg(feature = "kitty-graphics")]
mod images;
mod input;
mod paint;
mod search;

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, ElementInputHandler,
    EventEmitter, FocusHandle, Focusable, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight,
    IntoElement, KeyBinding, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Subscription,
    Task, Window, actions, canvas, div, prelude::*, px,
};
use lohtty_config::{Config, DEFAULT_FALLBACKS, FontConfig};
use lohtty_term::{PassThrough, Session, SessionEvent, TermSize, resolve_shell};
use lohtty_theme::Theme;

use crate::colors;
use crate::paint_now::PaintNow;
use paint::FramePainter;

actions!(terminal, [Copy, Paste, Search]);

const KEY_CONTEXT: &str = "Terminal";

/// Minimum interval between output-driven repaints while output streams.
/// The repaint itself lands on the next vsync, so 12 ms gives roughly 50-60
/// fps on 60-165 Hz displays: a screen changing faster can't be read anyway.
const MIN_OUTPUT_FRAME: Duration = Duration::from_millis(12);

/// Output arriving this soon after user input is treated as its echo and
/// painted immediately (see [`PaintNow`]); other output waits for vsync.
const ECHO_WINDOW: Duration = Duration::from_millis(250);

/// Events a terminal view reports to its container (tab / workspace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEvent {
    TitleChanged,
    /// The view gained keyboard focus (it becomes its tab's active pane).
    Focused,
    /// The shell exited; the pane should close.
    Exited,
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
        ]
    } else {
        vec![
            KeyBinding::new("ctrl-shift-c", Copy, ctx),
            KeyBinding::new("ctrl-shift-v", Paste, ctx),
            KeyBinding::new("ctrl-insert", Copy, ctx),
            KeyBinding::new("shift-insert", Paste, ctx),
            KeyBinding::new("ctrl-shift-f", Search, search_ctx),
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
}

/// A started shell, ready to be shown by a [`TerminalView`].
pub struct Spawned {
    session: Session,
    events: async_channel::Receiver<SessionEvent>,
    #[cfg(feature = "kitty-graphics")]
    graphics: Option<lohtty_term::kitty::Graphics>,
}

/// Starts the configured shell. Separate from [`TerminalView::new`] so that
/// failures surface before a view is created.
pub fn spawn_session(config: &Config) -> anyhow::Result<Spawned> {
    let shell = resolve_shell(config.shell.program.as_deref(), &config.shell.args);
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
            theme,
            font: config.font.clone(),
            focus,
            fonts: None,
            painted: Rc::default(),
            scroll_accum: 0.0,
            marked_text: None,
            deferred_input: None,
            selecting: false,
            reported_button: None,
            last_reported_cell: None,
            search: None,
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

    pub fn title(&self) -> &str {
        &self.title
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
                self.output_frame(cx)
            }
            SessionEvent::Title(title) => self.set_title(title, cx),
            SessionEvent::ResetTitle => self.set_title(self.default_title.clone(), cx),
            SessionEvent::Bell => {}
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
        }
        // Repaint: the cursor is hollow while unfocused.
        cx.notify();
    }

    /// The fonts and cell metrics for the current config, computed once.
    fn font_set(&mut self, window: &mut Window) -> FontSet {
        if let Some(set) = &self.fonts {
            return set.clone();
        }
        let fallbacks = resolve_fallbacks(&self.font, window);
        let make = |bold: bool, italic: bool| Font {
            family: self.font.family.clone().into(),
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

        let font_size = px(self.font.size);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&fonts[0]);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let set = FontSet {
            fonts,
            metrics: CellMetrics {
                font_size,
                cell_width,
                line_height: (font_size * 1.25).round(),
            },
        };
        self.fonts = Some(set.clone());
        set
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
}

/// The configured fallback list, or the installed subset of the defaults.
fn resolve_fallbacks(font: &FontConfig, window: &Window) -> Option<FontFallbacks> {
    static INSTALLED_DEFAULTS: OnceLock<Vec<String>> = OnceLock::new();
    let names = match &font.fallbacks {
        Some(list) => list.clone(),
        None => INSTALLED_DEFAULTS
            .get_or_init(|| {
                let installed: HashSet<String> =
                    window.text_system().all_font_names().into_iter().collect();
                let found: Vec<String> = DEFAULT_FALLBACKS
                    .iter()
                    .filter(|name| installed.contains(**name))
                    .map(|name| name.to_string())
                    .collect();
                tracing::info!(?found, "default font fallbacks");
                found
            })
            .clone(),
    };
    (!names.is_empty()).then(|| FontFallbacks::from_fonts(names))
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let FontSet { fonts, metrics } = self.font_set(window);
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
                        frame.paint(window, cx)
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
