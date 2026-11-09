//! The window's root view: tabs, each holding a tree of split panes.

mod render;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AppContext, Bounds, Context, Entity, EntityId, Focusable, IsZero, KeyBinding, Pixels,
    Subscription, Task, Window, actions,
};
use lohtty_config::{Config, ConfigWatcher};
use lohtty_theme::Theme;

use crate::pane_tree::{Axis, Direction, PaneTree, Removed, SplitId};
use crate::settings::{SettingsDialog, SettingsEvent};
use crate::terminal_view::{TerminalEvent, spawn_session};
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
    ]
);

const KEY_CONTEXT: &str = "Workspace";
const APP_TITLE: &str = "lohtty";

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
    _watcher: Option<ConfigWatcher>,
    _reload: Task<()>,
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
            _watcher: watcher,
            _reload: reload,
        };
        this.new_tab(&NewTab, window, cx);
        this
    }

    // --- Panes and tabs -------------------------------------------------------

    /// Starts a shell in a new terminal view, or records the error.
    fn new_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Pane> {
        let (session, events) = match spawn_session(&self.config) {
            Ok(spawned) => spawned,
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                self.error = Some(format!("Failed to start the shell: {e:#}"));
                cx.notify();
                return None;
            }
        };
        let (config, theme) = (&self.config, self.theme.clone());
        let pane = cx.new(|cx| TerminalView::new(session, events, config, theme, window, cx));
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
                self.update_window_title(window, cx);
                cx.notify();
            }
            TerminalEvent::Exited => self.close_pane_entity(pane, window, cx),
        }
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.new_pane(window, cx) else {
            return;
        };
        self.tabs.push(Tab {
            panes: PaneTree::new(pane.clone()),
            active: pane,
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
                if &tab.active == pane {
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
            tab.active = next;
            self.focus_active_pane(window, cx);
        }
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
