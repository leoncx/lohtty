//! Rendering: tab bar, split panes and draggable dividers.

use std::sync::OnceLock;

use gpui::{
    AnyElement, Context, CursorStyle, ElementId, IntoElement, MouseButton, MouseMoveEvent, Render,
    Rgba, SharedString, Window, WindowControlArea, canvas, div, prelude::*, px, relative, rgb,
    rgba,
};

use super::*;
use crate::CUSTOM_TITLEBAR;
use crate::colors::{mix, to_rgba};
use crate::pane_tree::Node;
use crate::tooltip::Tooltip;

/// Width of the divider hit area between panes, in pixels.
const DIVIDER: f32 = 5.0;
const TAB_BAR_HEIGHT: f32 = 34.0;

/// A 14×12 pane outline divided in two: side by side for [`Axis::Horizontal`]
/// (split right), stacked for [`Axis::Vertical`] (split down).
fn split_icon(axis: Axis, color: Rgba) -> impl IntoElement {
    let frame = div()
        .flex()
        .w(px(14.0))
        .h(px(12.0))
        .border_1()
        .border_color(color)
        .rounded(px(2.0));
    match axis {
        Axis::Horizontal => frame
            .flex_row()
            .child(div().flex_1())
            .child(div().w(px(1.0)).h_full().bg(color))
            .child(div().flex_1()),
        Axis::Vertical => frame
            .flex_col()
            .child(div().flex_1())
            .child(div().h(px(1.0)).w_full().bg(color))
            .child(div().flex_1()),
    }
}

/// Windows 11 caption glyphs, or the Windows 10 font with the same code points.
fn caption_icon_font(window: &Window) -> SharedString {
    static FONT: OnceLock<SharedString> = OnceLock::new();
    FONT.get_or_init(|| {
        let fluent = "Segoe Fluent Icons";
        if window
            .text_system()
            .all_font_names()
            .iter()
            .any(|f| f == fluent)
        {
            fluent.into()
        } else {
            "Segoe MDL2 Assets".into()
        }
    })
    .clone()
}

impl Workspace {
    /// Applies the configured background opacity to a color.
    fn translucent(&self, c: Rgba) -> Rgba {
        Rgba {
            a: self.config.window.opacity.clamp(0.0, 1.0),
            ..c
        }
    }

    /// The tab bar. With [`CUSTOM_TITLEBAR`] it is also the window's title bar:
    /// empty space drags the window and the caption buttons sit on the right.
    fn render_tab_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let bar = self.translucent(to_rgba(mix(t.background, t.foreground, 0.06)));
        let active_bg = self.translucent(to_rgba(t.background));
        let hover = to_rgba(mix(t.background, t.foreground, 0.12));
        let fg = to_rgba(t.foreground);
        let muted = to_rgba(mix(t.foreground, t.background, 0.45));
        let accent = to_rgba(t.ansi[4]);

        let tabs = div()
            .flex()
            .flex_row()
            .min_w_0()
            .flex_shrink()
            .overflow_hidden()
            .h_full()
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                let active = i == self.active_tab;
                let title = tab.active.read(cx).title().to_owned();
                div()
                    .id(("tab", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .pl_3()
                    .pr_1()
                    .h_full()
                    .min_w(px(90.0))
                    .max_w(px(220.0))
                    .flex_shrink()
                    .border_b_2()
                    .border_color(if active { accent } else { bar })
                    .bg(if active { active_bg } else { bar })
                    .text_color(if active { fg } else { muted })
                    .when(!active, |el| el.hover(|s| s.bg(hover)))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.activate_tab(i, window, cx)),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| this.close_tab_at(i, window, cx)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .id(("close-tab", i))
                            .px_1()
                            .rounded_sm()
                            .text_color(muted)
                            .hover(|s| s.bg(hover).text_color(fg))
                            .child("×")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(i, window, cx)
                            })),
                    )
            }))
            .child(
                div()
                    .id("new-tab")
                    .flex()
                    .items_center()
                    .px_3()
                    .h_full()
                    .text_color(muted)
                    .hover(|s| s.bg(hover).text_color(fg))
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx))),
            );

        div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(TAB_BAR_HEIGHT))
            .bg(bar)
            .text_sm()
            .child(self.inert_while_settings(tabs))
            .child(
                // Empty space. With the custom title bar it drags the window
                // (double-click maximizes, right-click opens the system menu);
                // a thin strip on top is left out so the top edge still resizes.
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(32.0))
                    .h_full()
                    .when(CUSTOM_TITLEBAR, |el| {
                        el.child(div().flex_none().h(px(4.0)))
                            .child(div().flex_1().window_control_area(WindowControlArea::Drag))
                    }),
            )
            .child(self.inert_while_settings(self.render_split_buttons(cx)))
            .child(self.render_settings_button(window, cx))
            .when(CUSTOM_TITLEBAR, |el| {
                el.child(self.render_caption_buttons(window))
            })
    }

    /// Title-bar controls that act on tabs or panes are dimmed and ignore the
    /// mouse while the settings dialog is open, like the keyboard shortcuts.
    /// The gear (refocuses the dialog), the drag area and the caption buttons
    /// stay usable.
    fn inert_while_settings(&self, content: impl IntoElement) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_row()
            .min_w_0()
            .flex_shrink()
            .h_full()
            .child(content)
            .when(self.settings.is_some(), |el| {
                el.opacity(0.4)
                    .child(div().absolute().top_0().left_0().size_full().occlude())
            })
    }

    /// Gear button opening the settings dialog.
    fn render_settings_button(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let hover = to_rgba(mix(t.background, t.foreground, 0.12));
        let color = to_rgba(mix(t.foreground, t.background, 0.25));
        let keys = if cfg!(target_os = "macos") {
            "Cmd+,"
        } else {
            "Ctrl+,"
        };
        // The Windows icon fonts have a proper gear (E713); elsewhere use the
        // Unicode one.
        let (glyph, font, size) = if cfg!(windows) {
            ("\u{E713}", Some(caption_icon_font(window)), px(13.0))
        } else {
            ("⚙", None, px(15.0))
        };
        div()
            .id("settings-button")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(36.0))
            .h_full()
            .text_color(color)
            .text_size(size)
            .when_some(font, |el, font| el.font_family(font))
            .hover(|s| s.bg(hover))
            .tooltip(Tooltip::text(format!("Settings ({keys})"), t))
            .child(glyph)
            .on_click(
                cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)),
            )
    }

    /// "Split right" and "split down" buttons, acting on the active pane.
    fn render_split_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let hover = to_rgba(mix(t.background, t.foreground, 0.12));
        let icon_color = to_rgba(mix(t.foreground, t.background, 0.25));
        let (right_keys, down_keys) = if cfg!(target_os = "macos") {
            ("Cmd+D", "Cmd+Shift+D")
        } else {
            ("Alt+Shift+=", "Alt+Shift+-")
        };
        let button = |id: &'static str, axis: Axis, tip: String| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(36.0))
                .h_full()
                .hover(|s| s.bg(hover))
                .tooltip(Tooltip::text(tip, t))
                .child(split_icon(axis, icon_color))
        };

        div()
            .flex()
            .flex_row()
            .flex_none()
            .h_full()
            .child(
                button(
                    "split-right",
                    Axis::Horizontal,
                    format!("Split right ({right_keys})"),
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.split_right(&SplitRight, window, cx)),
                ),
            )
            .child(
                button(
                    "split-down",
                    Axis::Vertical,
                    format!("Split down ({down_keys})"),
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.split_down(&SplitDown, window, cx)),
                ),
            )
    }

    /// Minimize / maximize-restore / close, drawn like Windows 11's. The OS
    /// handles the clicks (and the snap-layout flyout on maximize) through
    /// the control areas.
    fn render_caption_buttons(&self, window: &Window) -> impl IntoElement {
        let t = &self.theme;
        let hover = to_rgba(mix(t.background, t.foreground, 0.12));
        let fg = if window.is_window_active() {
            to_rgba(t.foreground)
        } else {
            to_rgba(mix(t.foreground, t.background, 0.45))
        };
        let font = caption_icon_font(window);
        let button = |id: &'static str, glyph: &'static str, area: WindowControlArea| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(46.0))
                .h_full()
                .font_family(font.clone())
                .text_size(px(10.0))
                .text_color(fg)
                .window_control_area(area)
                .child(glyph)
        };
        let maximize_glyph = if window.is_maximized() {
            "\u{E923}" // ChromeRestore
        } else {
            "\u{E922}" // ChromeMaximize
        };

        div()
            .flex()
            .flex_row()
            .flex_none()
            .h_full()
            .child(button("caption-min", "\u{E921}", WindowControlArea::Min).hover(|s| s.bg(hover)))
            .child(
                button("caption-max", maximize_glyph, WindowControlArea::Max)
                    .hover(|s| s.bg(hover)),
            )
            .child(
                button("caption-close", "\u{E8BB}", WindowControlArea::Close)
                    .hover(|s| s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))),
            )
    }

    fn render_node(&self, node: &Node<Pane>, cx: &mut Context<Self>) -> AnyElement {
        let split = match node {
            Node::Leaf(pane) => {
                return div()
                    .size_full()
                    .p(px(self.config.window.padding))
                    .child(pane.clone())
                    .into_any_element();
            }
            Node::Split(split) => split,
        };

        let (id, axis) = (split.id, split.axis);
        let bounds = self.split_bounds.clone();
        let mut container = div().relative().size_full().flex().child(
            // Records this split's bounds for divider dragging.
            canvas(
                move |b, _, _| {
                    bounds.borrow_mut().insert(id, b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        container = match axis {
            Axis::Horizontal => container.flex_row(),
            Axis::Vertical => container.flex_col(),
        };

        for (i, (child, ratio)) in split.children.iter().zip(&split.ratios).enumerate() {
            if i > 0 {
                container = container.child(self.render_divider(id, i - 1, axis, cx));
            }
            let cell = div()
                .flex_basis(relative(*ratio))
                .flex_shrink()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(self.render_node(child, cx));
            container = container.child(match axis {
                Axis::Horizontal => cell.h_full(),
                Axis::Vertical => cell.w_full(),
            });
        }
        container.into_any_element()
    }

    fn render_divider(
        &self,
        split: SplitId,
        index: usize,
        axis: Axis,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let line = to_rgba(mix(self.theme.background, self.theme.foreground, 0.25));
        let hit = div()
            .id(ElementId::Name(format!("divider-{split}-{index}").into()))
            .flex()
            .flex_none()
            .justify_center()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.start_drag(split, index);
                    cx.stop_propagation();
                }),
            );
        match axis {
            Axis::Horizontal => hit
                .w(px(DIVIDER))
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight)
                .child(div().w(px(1.0)).h_full().bg(line)),
            Axis::Vertical => hit
                .h(px(DIVIDER))
                .w_full()
                .cursor(CursorStyle::ResizeUpDown)
                .child(div().h(px(1.0)).w_full().bg(line)),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let body = match self.tabs.get(self.active_tab) {
            Some(tab) => self.render_node(tab.panes.root(), cx),
            None => div()
                .p_4()
                .child("No shell is running. Press Ctrl+Shift+T to retry.")
                .into_any_element(),
        };

        div()
            .key_context(KEY_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .text_color(to_rgba(theme.foreground))
            .font_family(crate::fonts::family(&self.config.font.family, window))
            .text_size(px(self.config.font.size))
            .on_action(cx.listener(Self::open_settings))
            // While the settings dialog is open, workspace shortcuts would act
            // on the panes behind it; only Ctrl+, (refocus the dialog) stays.
            .when(self.settings.is_none(), |el| {
                el.on_action(cx.listener(Self::new_tab))
                    .on_action(cx.listener(Self::close_tab))
                    .on_action(cx.listener(Self::next_tab))
                    .on_action(cx.listener(Self::prev_tab))
                    .on_action(cx.listener(Self::split_right))
                    .on_action(cx.listener(Self::split_down))
                    .on_action(cx.listener(Self::close_pane))
                    .on_action(cx.listener(|this, _: &FocusLeft, w, cx| {
                        this.focus_direction(Direction::Left, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &FocusRight, w, cx| {
                        this.focus_direction(Direction::Right, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &FocusUp, w, cx| {
                        this.focus_direction(Direction::Up, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &FocusDown, w, cx| {
                        this.focus_direction(Direction::Down, w, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &ActivateTab1, w, cx| this.activate_tab(0, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab2, w, cx| this.activate_tab(1, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab3, w, cx| this.activate_tab(2, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab4, w, cx| this.activate_tab(3, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab5, w, cx| this.activate_tab(4, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab6, w, cx| this.activate_tab(5, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab7, w, cx| this.activate_tab(6, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab8, w, cx| this.activate_tab(7, w, cx)),
                    )
                    // Like browsers: the 9th shortcut is always the last tab.
                    .on_action(cx.listener(|this, _: &ActivateTab9, w, cx| {
                        this.activate_tab(this.tabs.len().saturating_sub(1), w, cx)
                    }))
            })
            .on_mouse_move(
                cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.drag_to(ev.position, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.end_drag()),
            )
            .children(self.error.as_ref().map(|e| {
                div()
                    .px_2()
                    .py_1()
                    .bg(to_rgba(theme.ansi[1]))
                    .text_color(to_rgba(theme.background))
                    .child(e.clone())
            }))
            .child(self.render_tab_bar(window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .bg(self.translucent(to_rgba(theme.background)))
                    .child(body)
                    // The settings dialog covers the panes but not the title
                    // bar, so the window can still be moved or closed.
                    .children(self.settings.as_ref().map(|(dialog, _)| {
                        div()
                            .absolute()
                            .top(px(0.0))
                            .left(px(0.0))
                            .right(px(0.0))
                            .bottom(px(0.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(0x0000_0070))
                            .occlude()
                            .child(dialog.clone())
                    })),
            )
    }
}
