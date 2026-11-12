//! The window's root view: tabs, each holding a tree of split panes.

mod render;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Bounds, Context, Entity, EntityId, Focusable, IsZero, KeyBinding, Pixels,
    Subscription, Task, Window, actions,
};
use lohtty_config::{Config, ConfigWatcher, MIN_USAGE_REFRESH, UsageConfig};
use lohtty_term::notify::Notification;
use lohtty_term::status::{Kind, Shown, State};
use lohtty_theme::Theme;
use lohtty_usage::{Poller, PollerOptions, Provider, Update};

use crate::notifier::{self, Notifier, TaskbarProgress};
use crate::pane_tree::{Axis, Direction, PaneTree, Removed, SplitId};
use crate::settings::{SettingsDialog, SettingsEvent};
use crate::terminal_view::{TerminalEvent, spawn_session};
use crate::usage_meter::UsageBoard;
use crate::{TerminalView, load_theme, window_background};

actions!(
    workspace,
    [
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        SplitRight,
        SplitDown,
        ClosePane,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9,
        OpenSettings,
        TogglePaneZoom,
    ]
);

const KEY_CONTEXT: &str = "Workspace";
const APP_TITLE: &str = "lohtty";
/// Shortest time between two desktop notifications from the same pane.
const NOTIFY_INTERVAL: Duration = Duration::from_secs(5);

/// Default key bindings (close to Windows Terminal's on Windows and Linux).
pub fn key_bindings() -> Vec<KeyBinding> {
    let ctx = Some(KEY_CONTEXT);
    let mut bindings = if cfg!(target_os = "macos") {
        vec![
            KeyBinding::new("cmd-t", NewTab, ctx),
            KeyBinding::new("cmd-w", ClosePane, ctx),
            KeyBinding::new("cmd-d", SplitRight, ctx),
            KeyBinding::new("cmd-shift-d", SplitDown, ctx),
            KeyBinding::new("cmd-alt-left", FocusLeft, ctx),
            KeyBinding::new("cmd-alt-right", FocusRight, ctx),
            KeyBinding::new("cmd-alt-up", FocusUp, ctx),
            KeyBinding::new("cmd-alt-down", FocusDown, ctx),
            // iTerm2's "maximize active pane".
            KeyBinding::new("cmd-shift-enter", TogglePaneZoom, ctx),
        ]
    } else {
        vec![
            KeyBinding::new("ctrl-shift-t", NewTab, ctx),
            KeyBinding::new("ctrl-shift-w", ClosePane, ctx),
            // gpui on Windows folds Shift into the character (alt-+, alt-_);
            // the shift-= / shift-- forms cover platforms that keep it.
            KeyBinding::new("alt-shift-=", SplitRight, ctx),
            KeyBinding::new("alt-+", SplitRight, ctx),
            KeyBinding::new("alt-shift--", SplitDown, ctx),
            KeyBinding::new("alt-_", SplitDown, ctx),
            // Windows Terminal's Alt+Arrow, plus Ctrl+Alt+Arrow for setups where a
            // window manager (e.g. GlazeWM) captures Alt+Arrow globally.
            KeyBinding::new("alt-left", FocusLeft, ctx),
            KeyBinding::new("alt-right", FocusRight, ctx),
            KeyBinding::new("alt-up", FocusUp, ctx),
            KeyBinding::new("alt-down", FocusDown, ctx),
            KeyBinding::new("ctrl-alt-left", FocusLeft, ctx),
            KeyBinding::new("ctrl-alt-right", FocusRight, ctx),
            KeyBinding::new("ctrl-alt-up", FocusUp, ctx),
            KeyBinding::new("ctrl-alt-down", FocusDown, ctx),
            // WezTerm's pane zoom.
            KeyBinding::new("ctrl-shift-z", TogglePaneZoom, ctx),
        ]
    };
    bindings.extend([
        KeyBinding::new("ctrl-tab", NextTab, ctx),
        KeyBinding::new("ctrl-shift-tab", PrevTab, ctx),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-,"
            } else {
                "ctrl-,"
            },
            OpenSettings,
            ctx,
        ),
    ]);
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl-alt"
    };
    bindings.extend([
        KeyBinding::new(&format!("{modifier}-1"), ActivateTab1, ctx),
        KeyBinding::new(&format!("{modifier}-2"), ActivateTab2, ctx),
        KeyBinding::new(&format!("{modifier}-3"), ActivateTab3, ctx),
        KeyBinding::new(&format!("{modifier}-4"), ActivateTab4, ctx),
        KeyBinding::new(&format!("{modifier}-5"), ActivateTab5, ctx),
        KeyBinding::new(&format!("{modifier}-6"), ActivateTab6, ctx),
        KeyBinding::new(&format!("{modifier}-7"), ActivateTab7, ctx),
        KeyBinding::new(&format!("{modifier}-8"), ActivateTab8, ctx),
        KeyBinding::new(&format!("{modifier}-9"), ActivateTab9, ctx),
    ]);
    bindings
}

type Pane = Entity<TerminalView>;

struct Tab {
    panes: PaneTree<Pane>,
    active: Pane,
    /// The active pane fills the tab; the others keep running, hidden.
    zoomed: bool,
}

impl Tab {
    /// Whether the active pane is shown alone (only meaningful with splits).
    fn is_zoomed(&self) -> bool {
        self.zoomed && self.has_splits()
    }

    fn has_splits(&self) -> bool {
        matches!(self.panes.root(), crate::pane_tree::Node::Split(_))
    }
}

/// A divider being dragged: the split and the index of the child before it.
#[derive(Debug, Clone, Copy)]
struct DividerDrag {
    split: SplitId,
    index: usize,
}

pub struct Workspace {
    config: Config,
    theme: Theme,
    tabs: Vec<Tab>,
    active_tab: usize,
    /// Last error (config, theme or shell start), shown in a bar until cleared.
    error: Option<String>,
    /// Where each split was last laid out, for divider dragging.
    split_bounds: Rc<RefCell<HashMap<SplitId, Bounds<Pixels>>>>,
    dragging: Option<DividerDrag>,
    /// The open settings dialog, if any.
    settings: Option<(Entity<SettingsDialog>, Subscription)>,
    pane_subscriptions: HashMap<EntityId, Subscription>,
    /// Panes that asked for attention since they were last focused.
    attention: HashSet<EntityId>,
    /// When each pane last showed a desktop notification, to limit them.
    last_notified: HashMap<EntityId, Instant>,
    notifier: Notifier,
    /// Frame of the "working" spinner, and the timer advancing it while a
    /// pane reports work in progress.
    spinner_frame: usize,
    spinner: Option<Task<()>>,
    /// Coding agents' usage limits, shown on the title bar.
    usage: UsageBoard,
    usage_poller: Option<Poller>,
    /// Where pollers send their updates (a new poller starts when the
    /// `[usage]` settings change).
    usage_tx: async_channel::Sender<Update>,
    _usage_updates: Task<()>,
    /// Redraws the meter when the next limit window resets, so its ring
    /// drops to 0 without waiting for a poll.
    usage_reset_redraw: Option<Task<()>>,
    _watcher: Option<ConfigWatcher>,
    _reload: Task<()>,
    _notification_clicks: Task<()>,
    _activation: Subscription,
}

impl Workspace {
    pub fn new(config: Config, theme: Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded::<()>();
        let watcher = ConfigWatcher::spawn(move || {
            let _ = tx.try_send(());
        })
        .inspect_err(|e| tracing::warn!("config live reload disabled: {e}"))
        .ok();
        let reload = cx.spawn_in(window, async move |this, cx| {
            while rx.recv().await.is_ok() {
                // Editors emit several events per save: wait, then coalesce.
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                while rx.try_recv().is_ok() {}
                if this
                    .update_in(cx, |this, window, cx| this.reload(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        // A clicked notification brings its pane forward.
        let (notifier, clicks) = Notifier::new();
        let notification_clicks = cx.spawn_in(window, async move |this, cx| {
            while let Ok(token) = clicks.recv().await {
                let shown = this.update_in(cx, |this, window, cx| {
                    this.show_pane(token, window, cx);
                    window.activate_window();
                });
                if shown.is_err() {
                    break;
                }
            }
        });
        // Coming back to the window clears the focused pane's attention, and
        // its finished results count as seen.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active()
                && let Some(tab) = this.tabs.get(this.active_tab)
            {
                let pane = tab.active.clone();
                if this.attention.remove(&pane.entity_id()) {
                    cx.notify();
                }
                pane.update(cx, |view, cx| view.mark_seen(cx));
            }
        });

        // Usage limits, polled on a background thread.
        let (usage_tx, usage_rx) = async_channel::unbounded::<Update>();
        let usage_poller = spawn_usage_poller(&config.usage, &usage_tx);
        let usage_updates = cx.spawn(async move |this, cx| {
            while let Ok(update) = usage_rx.recv().await {
                let applied = this.update(cx, |this, cx| {
                    // A poller stopped after a settings change may still
                    // deliver a last update.
                    if usage_enabled(&this.config.usage, update.provider)
                        && this.usage.apply(update)
                    {
                        this.schedule_usage_redraw(cx);
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
        });

        let mut this = Self {
            config,
            theme,
            tabs: Vec::new(),
            active_tab: 0,
            error: None,
            split_bounds: Rc::default(),
            dragging: None,
            settings: None,
            pane_subscriptions: HashMap::new(),
            attention: HashSet::new(),
            last_notified: HashMap::new(),
            notifier,
            spinner_frame: 0,
            spinner: None,
            usage: UsageBoard::default(),
            usage_poller,
            usage_tx,
            _usage_updates: usage_updates,
            usage_reset_redraw: None,
            _watcher: watcher,
            _reload: reload,
            _notification_clicks: notification_clicks,
            _activation: activation,
        };
        this.new_tab(&NewTab, window, cx);
        this
    }

    // --- Panes and tabs -------------------------------------------------------

    /// Starts a shell in a new terminal view, or records the error.
    fn new_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Pane> {
        let spawned = match spawn_session(&self.config) {
            Ok(spawned) => spawned,
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                self.error = Some(format!("Failed to start the shell: {e:#}"));
                cx.notify();
                return None;
            }
        };
        let (config, theme) = (&self.config, self.theme.clone());
        let pane = cx.new(|cx| TerminalView::new(spawned, config, theme, window, cx));
        let subscription = cx.subscribe_in(&pane, window, Self::on_terminal_event);
        self.pane_subscriptions
            .insert(pane.entity_id(), subscription);
        Some(pane)
    }

    fn on_terminal_event(
        &mut self,
        pane: &Pane,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::TitleChanged => {
                self.update_window_title(window, cx);
                cx.notify();
            }
            TerminalEvent::Focused => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.panes.contains(pane)) {
                    tab.active = pane.clone();
                }
                self.attention.remove(&pane.entity_id());
                self.update_window_title(window, cx);
                cx.notify();
            }
            TerminalEvent::Exited => self.close_pane_entity(pane, window, cx),
            TerminalEvent::Attention(note) => {
                self.request_attention(pane, note.as_ref(), window, cx)
            }
            TerminalEvent::StatusChanged => {
                self.run_spinner(window, cx);
                cx.notify();
            }
        }
    }

    /// A pane asked for attention. Nothing happens if the user is looking at
    /// it; otherwise its tab is marked, and when lohtty isn't in front the
    /// taskbar button flashes and a desktop notification shows the message.
    fn request_attention(
        &mut self,
        pane: &Pane,
        note: Option<&Notification>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = window.is_window_active();
        if active && pane.focus_handle(cx).is_focused(window) {
            return;
        }
        self.attention.insert(pane.entity_id());
        cx.notify();
        if active {
            return;
        }
        notifier::flash(window);
        if !self.config.notifications.desktop {
            return;
        }
        // At most one notification per pane every few seconds (bells repeat).
        let now = Instant::now();
        if self
            .last_notified
            .get(&pane.entity_id())
            .is_some_and(|at| now.duration_since(*at) < NOTIFY_INTERVAL)
        {
            return;
        }
        self.last_notified.insert(pane.entity_id(), now);
        let pane_title = pane.read(cx).title().to_owned();
        let (title, body) = match note {
            Some(note) => (
                note.title.clone().unwrap_or_else(|| pane_title.clone()),
                note.body.clone(),
            ),
            None => (pane_title, "Needs your attention".to_owned()),
        };
        self.notifier
            .notify(&title, &body, pane.entity_id().as_u64());
    }

    /// Brings the pane with this entity id forward: its tab, then the pane.
    fn show_pane(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, pane)) = self.tabs.iter().enumerate().find_map(|(i, tab)| {
            tab.panes
                .panes()
                .into_iter()
                .find(|p| p.entity_id().as_u64() == id)
                .map(|p| (i, p))
        }) else {
            return;
        };
        let tab = &mut self.tabs[index];
        if tab.active != pane {
            // Another pane than the zoomed one: show the splits again.
            tab.zoomed = false;
        }
        tab.active = pane;
        self.activate_tab(index, window, cx);
    }

    /// The status a tab shows: the most urgent of its panes' records.
    fn tab_status(&self, tab: &Tab, cx: &App) -> Option<Shown> {
        tab.panes
            .panes()
            .iter()
            .filter_map(|p| p.read(cx).status().into_iter().next())
            .max_by_key(|s| s.record.state.urgency())
    }

    /// The taskbar shows the most urgent status of all tabs, so work in a
    /// background tab shows while lohtty is minimized.
    fn taskbar_progress(&self, cx: &App) -> TaskbarProgress {
        let top = self
            .tabs
            .iter()
            .filter_map(|tab| self.tab_status(tab, cx))
            .max_by_key(|s| s.record.state.urgency());
        let Some(Shown { record, .. }) = top else {
            return TaskbarProgress::None;
        };
        match record.state {
            State::Working => record
                .progress
                .map_or(TaskbarProgress::Indeterminate, TaskbarProgress::Normal),
            State::Blocked => TaskbarProgress::Paused(record.progress.unwrap_or(100)),
            State::Error => TaskbarProgress::Error,
            State::Done | State::Idle => TaskbarProgress::None,
        }
    }

    fn any_working(&self, cx: &App) -> bool {
        self.tabs.iter().any(|tab| {
            tab.panes.panes().iter().any(|p| {
                p.read(cx)
                    .status()
                    .iter()
                    .any(|s| s.record.state == State::Working)
            })
        })
    }

    /// Advances the spinner ten times a second while a pane is working;
    /// the timer stops when none is.
    fn run_spinner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spinner.is_some() || !self.any_working(cx) {
            return;
        }
        self.spinner = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let more = this.update(cx, |this, cx| {
                    if !this.any_working(cx) {
                        this.spinner = None;
                        return false;
                    }
                    this.spinner_frame = this.spinner_frame.wrapping_add(1);
                    cx.notify();
                    true
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Whether a tab has a pane waiting for the user.
    fn tab_needs_attention(&self, tab: &Tab) -> bool {
        tab.panes
            .panes()
            .iter()
            .any(|p| self.attention.contains(&p.entity_id()))
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.new_pane(window, cx) else {
            return;
        };
        self.tabs.push(Tab {
            panes: PaneTree::new(pane.clone()),
            active: pane,
            zoomed: false,
        });
        self.activate_tab(self.tabs.len() - 1, window, cx);
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.active_tab = index;
        self.focus_active_pane(window, cx);
    }

    fn focus_active_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            window.focus(&tab.active.focus_handle(cx));
        }
        self.update_window_title(window, cx);
        cx.notify();
    }

    fn update_window_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .tabs
            .get(self.active_tab)
            .map(|tab| tab.active.read(cx).title().to_owned());
        window.set_window_title(title.as_deref().unwrap_or(APP_TITLE));
    }

    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        for pane in tab.panes.panes() {
            self.pane_subscriptions.remove(&pane.entity_id());
        }
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        if self.active_tab > index || self.active_tab >= self.tabs.len() {
            self.active_tab = self.active_tab.saturating_sub(1);
        }
        self.focus_active_pane(window, cx);
    }

    fn close_pane_entity(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.panes.contains(pane)) else {
            return;
        };
        let tab = &mut self.tabs[index];
        // The pane that takes focus: a neighbor, else the first remaining pane.
        let successor = [
            Direction::Left,
            Direction::Up,
            Direction::Right,
            Direction::Down,
        ]
        .into_iter()
        .find_map(|d| tab.panes.neighbor(pane, d));
        match tab.panes.remove(pane) {
            Removed::LastPane => self.close_tab_at(index, window, cx),
            Removed::NotFound => {}
            Removed::Yes => {
                self.pane_subscriptions.remove(&pane.entity_id());
                self.attention.remove(&pane.entity_id());
                self.last_notified.remove(&pane.entity_id());
                if &tab.active == pane {
                    // The zoomed pane is gone: show the others again.
                    tab.zoomed = false;
                    tab.active = successor.unwrap_or_else(|| tab.panes.panes()[0].clone());
                }
                if index == self.active_tab {
                    self.focus_active_pane(window, cx);
                }
                cx.notify();
            }
        }
    }

    // --- Actions --------------------------------------------------------------

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len().max(1);
        self.activate_tab((self.active_tab + 1) % n, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len().max(1);
        self.activate_tab((self.active_tab + n - 1) % n, window, cx);
    }

    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let Some(pane) = self.new_pane(window, cx) else {
            return;
        };
        let tab = &mut self.tabs[self.active_tab];
        let target = tab.active.clone();
        tab.panes.split(&target, axis, pane.clone());
        // The new pane must be visible.
        tab.zoomed = false;
        tab.active = pane;
        self.focus_active_pane(window, cx);
    }

    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Horizontal, window, cx);
    }

    fn split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Vertical, window, cx);
    }

    fn close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let pane = tab.active.clone();
            self.close_pane_entity(&pane, window, cx);
        }
    }

    fn focus_direction(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if let Some(next) = tab.panes.neighbor(&tab.active, direction) {
            // Like tmux: moving to another pane leaves the zoom.
            tab.zoomed = false;
            tab.active = next;
            self.focus_active_pane(window, cx);
        }
    }

    /// Shows the active pane alone, filling the tab, or the splits again.
    /// Nothing happens in a tab with a single pane.
    fn toggle_pane_zoom(
        &mut self,
        _: &TogglePaneZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if !tab.has_splits() {
            return;
        }
        tab.zoomed = !tab.zoomed;
        self.focus_active_pane(window, cx);
    }

    // --- Divider dragging -------------------------------------------------------

    fn start_drag(&mut self, split: SplitId, index: usize) {
        self.dragging = Some(DividerDrag { split, index });
    }

    fn drag_to(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.dragging else { return };
        let Some(bounds) = self.split_bounds.borrow().get(&drag.split).copied() else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if bounds.size.width.is_zero() || bounds.size.height.is_zero() {
            return;
        }
        let Some(axis) = tab.panes.axis_of(drag.split) else {
            return;
        };
        let fraction = match axis {
            Axis::Horizontal => (position.x - bounds.origin.x) / bounds.size.width,
            Axis::Vertical => (position.y - bounds.origin.y) / bounds.size.height,
        };
        tab.panes.resize(drag.split, drag.index, fraction);
        cx.notify();
    }

    // --- Settings dialog -----------------------------------------------------

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((dialog, _)) = &self.settings {
            window.focus(&dialog.focus_handle(cx));
            return;
        }
        let (config, theme) = (self.config.clone(), self.theme.clone());
        let dialog = cx.new(|cx| SettingsDialog::new(config, theme, window, cx));
        let subscription = cx.subscribe_in(&dialog, window, Self::on_settings_event);
        window.focus(&dialog.focus_handle(cx));
        self.settings = Some((dialog, subscription));
        cx.notify();
    }

    fn on_settings_event(
        &mut self,
        _dialog: &Entity<SettingsDialog>,
        event: &SettingsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings = None;
        if matches!(event, SettingsEvent::Saved) {
            // The watcher would pick the change up too; reloading now makes it
            // immediate and works even if watching is unavailable.
            self.reload(window, cx);
        }
        self.focus_active_pane(window, cx);
    }

    fn end_drag(&mut self) {
        self.dragging = None;
    }

    // --- Config reload ----------------------------------------------------------

    /// Reloads config and theme from disk. On error, keeps the current ones.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = Config::load()
            .map_err(|e| e.to_string())
            .and_then(|config| {
                let theme = load_theme(&config).map_err(|e| e.to_string())?;
                Ok((config, theme))
            });
        let (config, theme) = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                tracing::error!("config not reloaded: {e}");
                self.error = Some(format!("Config error (previous settings kept): {e}"));
                cx.notify();
                return;
            }
        };

        tracing::info!(theme = %theme.name, "config reloaded");
        if config.usage != self.config.usage {
            // Stop the old poller before starting the new one.
            self.usage_poller = None;
            self.usage_poller = spawn_usage_poller(&config.usage, &self.usage_tx);
            self.usage
                .retain(|provider| usage_enabled(&config.usage, provider));
            self.schedule_usage_redraw(cx);
        }
        window.set_background_appearance(window_background(&config.window));
        for pane in self.tabs.iter().flat_map(|t| t.panes.panes()) {
            pane.update(cx, |view, cx| view.apply_config(&config, theme.clone(), cx));
        }
        self.config = config;
        self.theme = theme;
        self.error = None;
        cx.notify();
    }
}

impl Workspace {
    /// Sets a timer for the meter's next reset, replacing any earlier one.
    fn schedule_usage_redraw(&mut self, cx: &mut Context<Self>) {
        let now = std::time::SystemTime::now();
        self.usage_reset_redraw = self.usage.next_reset(now).map(|at| {
            // A second late, so the window has reset when it redraws.
            let wait = at.duration_since(now).unwrap_or_default() + Duration::from_secs(1);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| {
                    this.schedule_usage_redraw(cx);
                    cx.notify();
                });
            })
        });
    }
}

fn usage_enabled(usage: &UsageConfig, provider: Provider) -> bool {
    match provider {
        Provider::Claude => usage.claude,
        Provider::Codex => usage.codex,
    }
}

/// A poller for the enabled agents, or none if the meter is off.
fn spawn_usage_poller(usage: &UsageConfig, tx: &async_channel::Sender<Update>) -> Option<Poller> {
    if !usage.claude && !usage.codex {
        return None;
    }
    let options = PollerOptions {
        claude: usage.claude,
        codex: usage.codex,
        claude_interval: Duration::from_secs(usage.refresh.max(MIN_USAGE_REFRESH)),
        ..PollerOptions::default()
    };
    let tx = tx.clone();
    Poller::spawn(options, move |update| {
        let _ = tx.try_send(update);
    })
    .inspect_err(|e| tracing::warn!("usage meter disabled: {e}"))
    .ok()
}
