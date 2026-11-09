//! Rendering: tab bar, split panes and draggable dividers.

use gpui::{
    AnyElement, Context, CursorStyle, ElementId, IntoElement, MouseButton, MouseMoveEvent, Render,
    Rgba, Window, canvas, div, prelude::*, px, relative,
};

use super::*;
use crate::colors::{mix, to_rgba};
use crate::pane_tree::Node;

/// Width of the divider hit area between panes, in pixels.
const DIVIDER: f32 = 5.0;
const TAB_BAR_HEIGHT: f32 = 34.0;

impl Workspace {
    /// Applies the configured background opacity to a color.
    fn translucent(&self, c: Rgba) -> Rgba {
        Rgba {
            a: self.config.window.opacity.clamp(0.0, 1.0),
            ..c
        }
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let bar = self.translucent(to_rgba(mix(t.background, t.foreground, 0.06)));
        let active_bg = self.translucent(to_rgba(t.background));
        let hover = to_rgba(mix(t.background, t.foreground, 0.12));
        let fg = to_rgba(t.foreground);
        let muted = to_rgba(mix(t.foreground, t.background, 0.45));
        let accent = to_rgba(t.ansi[4]);

        div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(TAB_BAR_HEIGHT))
            .bg(bar)
            .text_sm()
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .font_family(self.config.font.family.clone())
            .text_size(px(self.config.font.size))
            .on_action(cx.listener(Self::new_tab))
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
            .on_action(
                cx.listener(|this, _: &FocusUp, w, cx| this.focus_direction(Direction::Up, w, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusDown, w, cx| {
                this.focus_direction(Direction::Down, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ActivateTab1, w, cx| this.activate_tab(0, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab2, w, cx| this.activate_tab(1, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab3, w, cx| this.activate_tab(2, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab4, w, cx| this.activate_tab(3, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab5, w, cx| this.activate_tab(4, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab6, w, cx| this.activate_tab(5, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab7, w, cx| this.activate_tab(6, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab8, w, cx| this.activate_tab(7, w, cx)))
            // Like browsers: the 9th shortcut is always the last tab.
            .on_action(cx.listener(|this, _: &ActivateTab9, w, cx| {
                this.activate_tab(this.tabs.len().saturating_sub(1), w, cx)
            }))
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
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(self.translucent(to_rgba(theme.background)))
                    .child(body),
            )
    }
}
