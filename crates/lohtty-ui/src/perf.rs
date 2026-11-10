//! Frame timing counters, reported every 2 s at debug level under the
//! `lohtty_ui::perf` target (e.g. `LOHTTY_LOG=lohtty_ui::perf=debug`).
//! Recording is a few `Instant` reads per frame; reporting is skipped
//! entirely unless that target is enabled.

use std::cell::RefCell;
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Stats {
    frames: u32,
    grid: Duration,
    shape: Duration,
    paint: Duration,
    segments: usize,
    since: Option<Instant>,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::default();
}

/// Per-frame phase timings from the terminal painter.
pub struct FrameTimings {
    /// Walking the grid under the terminal lock (colors, segments, cursor).
    pub grid: Duration,
    /// Shaping text segments into glyph runs.
    pub shape: Duration,
    pub segments: usize,
}

pub fn record_prepaint(t: FrameTimings) {
    if !enabled() {
        return;
    }
    STATS.with_borrow_mut(|s| {
        s.frames += 1;
        s.grid += t.grid;
        s.shape += t.shape;
        s.segments += t.segments;
    });
}

pub fn record_paint(paint: Duration) {
    if !enabled() {
        return;
    }
    STATS.with_borrow_mut(|s| {
        s.paint += paint;
        let now = Instant::now();
        let since = *s.since.get_or_insert(now);
        let elapsed = now - since;
        if elapsed >= REPORT_EVERY && s.frames > 0 {
            let per_frame = |d: Duration| d.as_secs_f64() * 1e3 / s.frames as f64;
            let busy = (s.grid + s.shape + s.paint).as_secs_f64() / elapsed.as_secs_f64();
            tracing::debug!(
                target: "lohtty_ui::perf",
                fps = format_args!("{:.1}", s.frames as f64 / elapsed.as_secs_f64()),
                grid_ms = format_args!("{:.2}", per_frame(s.grid)),
                shape_ms = format_args!("{:.2}", per_frame(s.shape)),
                paint_ms = format_args!("{:.2}", per_frame(s.paint)),
                segments = s.segments / s.frames as usize,
                core_pct = format_args!("{:.0}", busy * 100.0),
                "frame timings"
            );
            *s = Stats {
                since: Some(now),
                ..Stats::default()
            };
        }
    });
}

fn enabled() -> bool {
    tracing::enabled!(target: "lohtty_ui::perf", tracing::Level::DEBUG)
}

/// Typing latency inside lohtty, logged per keystroke at debug level under
/// `lohtty_ui::latency`: input sent to the shell → first output back (the
/// shell's echo, outside lohtty) → the frame showing it painted.
#[derive(Default)]
struct Latency {
    sent: Option<Instant>,
    echoed: Option<Instant>,
}

thread_local! {
    static LATENCY: RefCell<Latency> = RefCell::default();
}

fn latency_enabled() -> bool {
    tracing::enabled!(target: "lohtty_ui::latency", tracing::Level::DEBUG)
}

/// User input was written to the PTY.
pub fn input_sent() {
    if latency_enabled() {
        LATENCY.with_borrow_mut(|l| {
            *l = Latency {
                sent: Some(Instant::now()),
                echoed: None,
            }
        });
    }
}

/// The UI thread received an output wakeup.
pub fn output_received() {
    if latency_enabled() {
        LATENCY.with_borrow_mut(|l| {
            if l.sent.is_some() && l.echoed.is_none() {
                l.echoed = Some(Instant::now());
            }
        });
    }
}

/// A frame finished painting (recorded into gpui's scene; presenting follows).
pub fn frame_painted() {
    if latency_enabled() {
        LATENCY.with_borrow_mut(|l| {
            if let (Some(sent), Some(echoed)) = (l.sent, l.echoed) {
                let now = Instant::now();
                tracing::debug!(
                    target: "lohtty_ui::latency",
                    echo_ms = format_args!("{:.2}", (echoed - sent).as_secs_f64() * 1e3),
                    to_paint_ms = format_args!("{:.2}", (now - echoed).as_secs_f64() * 1e3),
                    total_ms = format_args!("{:.2}", (now - sent).as_secs_f64() * 1e3),
                    "keystroke"
                );
                *l = Latency::default();
            }
        });
    }
}
