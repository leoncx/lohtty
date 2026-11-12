//! The usage meter: for each coding agent set up on the machine, a donut on
//! the title bar (outer ring: the 5-hour session, inner ring: the week) and a
//! tooltip with each window's use and the time left until it resets.

use std::f32::consts::TAU;
use std::time::SystemTime;

use gpui::{
    AnyView, App, AppContext, Context, FontWeight, IntoElement, PathBuilder, Render, Rgba,
    SharedString, Window, canvas, div, point, prelude::*, px,
};
use lohtty_theme::Theme;
use lohtty_usage::{
    Level, Provider, ProviderState, Update, UsageError, UsageSnapshot, UsageWindow, humanize_ago,
    humanize_remaining,
};

use crate::colors::{mix, to_rgba};

/// Side of the donut, in pixels.
const DONUT: f32 = 22.0;
/// Outer (session) ring: radius to the middle of the stroke, and width.
const OUTER: (f32, f32) = (9.25, 2.5);
/// Inner (week) ring.
const INNER: (f32, f32) = (5.75, 2.0);

/// What is known about one provider.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// The last good numbers, kept while later polls fail.
    pub snapshot: Option<UsageSnapshot>,
    /// Why the last poll failed, if it did.
    pub error: Option<UsageError>,
}

/// The providers to show, in a fixed order (Claude, then Codex).
#[derive(Debug, Default)]
pub struct UsageBoard {
    entries: Vec<(Provider, Entry)>,
}

impl UsageBoard {
    /// Applies a poller update; returns whether anything changed.
    pub fn apply(&mut self, update: Update) -> bool {
        let index = self.entries.iter().position(|(p, _)| *p == update.provider);
        let before = index.map(|i| self.entries[i].1.clone());
        match update.state {
            ProviderState::Unavailable => {
                if let Some(i) = index {
                    self.entries.remove(i);
                }
            }
            state => {
                let i = index.unwrap_or_else(|| {
                    self.entries.push((update.provider, Entry::default()));
                    self.entries.sort_by_key(|(p, _)| order(*p));
                    self.entries
                        .iter()
                        .position(|(p, _)| *p == update.provider)
                        .unwrap_or_default()
                });
                let entry = &mut self.entries[i].1;
                match state {
                    ProviderState::Ready(snapshot) => {
                        entry.snapshot = Some(snapshot);
                        entry.error = None;
                    }
                    ProviderState::Failed(error) => entry.error = Some(error),
                    ProviderState::Unavailable => unreachable!(),
                }
            }
        }
        let after = self
            .entries
            .iter()
            .find(|(p, _)| *p == update.provider)
            .map(|(_, e)| e.clone());
        before != after
    }

    /// Keeps only the providers `keep` accepts.
    pub fn retain(&mut self, keep: impl Fn(Provider) -> bool) {
        self.entries.retain(|(p, _)| keep(*p));
    }

    /// The soonest reset time still to come, across all shown windows.
    pub fn next_reset(&self, now: SystemTime) -> Option<SystemTime> {
        self.entries
            .iter()
            .filter_map(|(_, e)| e.snapshot.as_ref())
            .flat_map(|s| &s.windows)
            .filter_map(|w| w.resets_at)
            .filter(|at| *at > now)
            .min()
    }

    pub fn entries(&self) -> &[(Provider, Entry)] {
        &self.entries
    }
}

fn order(provider: Provider) -> u8 {
    match provider {
        Provider::Claude => 0,
        Provider::Codex => 1,
    }
}

/// The meter's colors, from the theme.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    fg: Rgba,
    muted: Rgba,
    track: Rgba,
    accent: Rgba,
    warn: Rgba,
    critical: Rgba,
    tip_bg: Rgba,
    tip_border: Rgba,
}

impl Palette {
    pub fn new(theme: &Theme) -> Palette {
        Palette {
            fg: to_rgba(theme.foreground),
            muted: to_rgba(mix(theme.foreground, theme.background, 0.45)),
            track: to_rgba(mix(theme.background, theme.foreground, 0.18)),
            accent: to_rgba(theme.ansi[4]),
            warn: to_rgba(theme.ansi[3]),
            critical: to_rgba(theme.ansi[1]),
            tip_bg: to_rgba(mix(theme.background, theme.foreground, 0.1)),
            tip_border: to_rgba(mix(theme.background, theme.foreground, 0.25)),
        }
    }

    /// The session ring and bars: the accent, then yellow and red.
    fn level(&self, used: f32) -> Rgba {
        match Level::of(used) {
            Level::Normal => self.accent,
            Level::Warn => self.warn,
            Level::Critical => self.critical,
        }
    }

    /// The week ring: muted until it warns.
    fn week_level(&self, used: f32) -> Rgba {
        match Level::of(used) {
            Level::Normal => self.muted,
            _ => self.level(used),
        }
    }
}

/// A short mark in the donut's hole telling the providers apart.
fn mark(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "\u{273B}", // ✻
        Provider::Codex => "\u{276F}",  // ❯
    }
}

/// Points along a circle from 12 o'clock, clockwise, covering `fraction`
/// of the turn (clamped to 0..=1).
pub fn arc_points(center: (f32, f32), radius: f32, fraction: f32) -> Vec<(f32, f32)> {
    let fraction = fraction.clamp(0.0, 1.0);
    let segments = ((64.0 * fraction).ceil() as usize).max(2);
    (0..=segments)
        .map(|i| {
            let angle = TAU * fraction * i as f32 / segments as f32;
            (
                center.0 + radius * angle.sin(),
                center.1 - radius * angle.cos(),
            )
        })
        .collect()
}

/// The donut for one provider. Rings show the use now (0 once a window has
/// reset); a failed last poll dims it and shows `!`.
pub fn donut(provider: Provider, entry: &Entry, palette: Palette) -> impl IntoElement {
    let now = SystemTime::now();
    let snapshot = entry.snapshot.as_ref();
    let session = snapshot
        .and_then(UsageSnapshot::session)
        .map(|w| w.used_at(now));
    let week = snapshot
        .and_then(UsageSnapshot::week)
        .map(|w| w.used_at(now));
    let stale = entry.error.is_some();
    let rings = [
        (OUTER, session, session.map(|u| palette.level(u))),
        (INNER, week, week.map(|u| palette.week_level(u))),
    ];
    let (glyph, glyph_color) = if stale && snapshot.is_none() {
        ("!", palette.warn)
    } else {
        (mark(provider), palette.muted)
    };

    div()
        .relative()
        .flex_none()
        .size(px(DONUT))
        .when(stale, |el| el.opacity(0.55))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let origin = bounds.origin;
                    let center = (DONUT / 2.0, DONUT / 2.0);
                    let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
                    let stroke =
                        |points: &[(f32, f32)], width: f32, color: Rgba, window: &mut Window| {
                            let mut path = PathBuilder::stroke(px(width));
                            path.move_to(at(points[0]));
                            for p in &points[1..] {
                                path.line_to(at(*p));
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        };
                    for ((radius, width), used, color) in rings {
                        stroke(
                            &arc_points(center, radius, 1.0),
                            width,
                            palette.track,
                            window,
                        );
                        if let (Some(used), Some(color)) = (used, color)
                            && used > 0.0
                        {
                            stroke(
                                &arc_points(center, radius, used / 100.0),
                                width,
                                color,
                                window,
                            );
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(7.0))
                .line_height(px(7.0))
                .text_color(glyph_color)
                .child(glyph),
        )
}

/// "resets in 2 h 13 min", "reset since", or nothing if unknown.
pub fn reset_text(window: &UsageWindow, now: SystemTime) -> String {
    match window.remaining(now) {
        Some(left) => format!("resets in {}", humanize_remaining(left)),
        None if window.has_reset(now) => "reset since".to_owned(),
        None => String::new(),
    }
}

/// The tooltip of a provider's donut.
pub struct UsageTooltip {
    provider: Provider,
    entry: Entry,
    palette: Palette,
}

impl UsageTooltip {
    /// A tooltip builder for [`gpui::StatefulInteractiveElement::tooltip`].
    pub fn build(
        provider: Provider,
        entry: Entry,
        palette: Palette,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |_, cx| {
            let entry = entry.clone();
            cx.new(|_| UsageTooltip {
                provider,
                entry,
                palette,
            })
            .into()
        }
    }
}

impl Render for UsageTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let now = SystemTime::now();
        let snapshot = self.entry.snapshot.as_ref();

        let title = match snapshot.and_then(|s| s.plan.as_deref()) {
            Some(plan) => format!("{} \u{00B7} {plan}", self.provider.name()),
            None => self.provider.name().to_owned(),
        };
        let rows = snapshot
            .map(|s| s.windows.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|w| {
                let used = w.used_at(now);
                let color = p.level(used);
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(76.0)).child(SharedString::from(w.label())))
                    .child(
                        div()
                            .w(px(34.0))
                            .flex()
                            .justify_end()
                            .text_color(color)
                            .child(format!("{used:.0}%")),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(p.track)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(color)
                                    .w(px(64.0 * used / 100.0)),
                            ),
                    )
                    .child(div().text_color(p.muted).child(reset_text(&w, now)))
            });
        let updated = snapshot.map(|s| {
            let ago = humanize_ago(now.duration_since(s.observed_at).unwrap_or_default());
            match self.provider {
                // Codex reports its limits only when it runs.
                Provider::Codex => format!("As of the last Codex turn, {ago}"),
                Provider::Claude => format!("Updated {ago}"),
            }
        });
        let error = self.entry.error.as_ref().map(|e| {
            if snapshot.is_some() {
                format!("Couldn't update: {e}")
            } else {
                format!("No data: {e}")
            }
        });

        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(p.tip_border)
            .bg(p.tip_bg)
            .text_color(p.fg)
            .text_xs()
            .shadow_md()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .children(rows)
            .children(updated.map(|u| div().text_color(p.muted).child(u)))
            .children(error.map(|e| div().max_w(px(320.0)).text_color(p.warn).child(e)))
            .child(div().text_color(p.muted).child("Click to refresh"))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lohtty_usage::WindowKind;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn snapshot(provider: Provider, session: f32) -> UsageSnapshot {
        UsageSnapshot {
            provider,
            plan: None,
            windows: vec![
                UsageWindow {
                    kind: WindowKind::Session,
                    scope: None,
                    used: session,
                    resets_at: Some(at(1000)),
                },
                UsageWindow {
                    kind: WindowKind::Week,
                    scope: None,
                    used: 50.0,
                    resets_at: None,
                },
            ],
            observed_at: at(0),
        }
    }

    fn update(provider: Provider, state: ProviderState) -> Update {
        Update { provider, state }
    }

    #[test]
    fn board_keeps_order_and_last_good_numbers() {
        let mut board = UsageBoard::default();
        let codex = snapshot(Provider::Codex, 10.0);
        assert!(board.apply(update(Provider::Codex, ProviderState::Ready(codex.clone()))));
        assert!(!board.apply(update(Provider::Codex, ProviderState::Ready(codex))));
        let claude = snapshot(Provider::Claude, 80.0);
        assert!(board.apply(update(
            Provider::Claude,
            ProviderState::Ready(claude.clone())
        )));
        let order: Vec<_> = board.entries().iter().map(|(p, _)| *p).collect();
        assert_eq!(order, [Provider::Claude, Provider::Codex]);

        // A failure keeps the snapshot and adds the error.
        assert!(board.apply(update(
            Provider::Claude,
            ProviderState::Failed(UsageError::Status(500))
        )));
        let entry = &board.entries()[0].1;
        assert_eq!(entry.snapshot.as_ref(), Some(&claude));
        assert_eq!(entry.error, Some(UsageError::Status(500)));

        // Success clears the error; unavailable removes the provider.
        assert!(board.apply(update(Provider::Claude, ProviderState::Ready(claude))));
        assert_eq!(board.entries()[0].1.error, None);
        assert!(board.apply(update(Provider::Claude, ProviderState::Unavailable)));
        assert!(!board.apply(update(Provider::Claude, ProviderState::Unavailable)));
        assert_eq!(board.entries().len(), 1);

        board.retain(|p| p != Provider::Codex);
        assert!(board.entries().is_empty());
    }

    #[test]
    fn next_reset_is_the_soonest_to_come() {
        let mut board = UsageBoard::default();
        assert_eq!(board.next_reset(at(0)), None);
        // Session resets at 1000; the week's reset is unknown.
        board.apply(update(
            Provider::Codex,
            ProviderState::Ready(snapshot(Provider::Codex, 10.0)),
        ));
        let mut claude = snapshot(Provider::Claude, 10.0);
        claude.windows[0].resets_at = Some(at(500));
        board.apply(update(Provider::Claude, ProviderState::Ready(claude)));
        assert_eq!(board.next_reset(at(0)), Some(at(500)));
        assert_eq!(board.next_reset(at(500)), Some(at(1000)));
        assert_eq!(board.next_reset(at(1000)), None);
    }

    #[test]
    fn a_failure_without_numbers_still_shows() {
        let mut board = UsageBoard::default();
        board.apply(update(
            Provider::Claude,
            ProviderState::Failed(UsageError::Expired),
        ));
        assert_eq!(board.entries().len(), 1);
        assert_eq!(board.entries()[0].1.snapshot, None);
    }

    #[test]
    fn arcs_start_at_twelve_and_turn_clockwise() {
        let close =
            |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4;
        let quarter = arc_points((10.0, 10.0), 5.0, 0.25);
        assert!(close(quarter[0], (10.0, 5.0)));
        assert!(close(*quarter.last().unwrap(), (15.0, 10.0)));
        let full = arc_points((0.0, 0.0), 1.0, 2.0);
        assert!(close(full[0], *full.last().unwrap()));
        assert_eq!(full.len(), 65);
        assert_eq!(arc_points((0.0, 0.0), 1.0, 0.0).len(), 3);
    }

    #[test]
    fn reset_texts() {
        let s = snapshot(Provider::Claude, 80.0);
        assert_eq!(reset_text(&s.windows[0], at(800)), "resets in 3 min");
        assert_eq!(reset_text(&s.windows[0], at(1000)), "reset since");
        assert_eq!(reset_text(&s.windows[1], at(0)), "");
    }
}
