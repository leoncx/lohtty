//! Animation frames of an image (`a=f`, `a=a`, `a=c`, `d=f`), following
//! kitty's behavior.
//!
//! Frames are numbered from 1 in the protocol; frame 1 is the root frame,
//! the image's own data. Every frame is kept fully composed (no chains of
//! deltas), so showing one is a pointer swap. The animation advances only
//! when the image is looked up for drawing, one shown frame per gap, as
//! kitty's timer does: an image off screen doesn't animate.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Gap of a new frame when the client gives none (`z=0` or no `z`).
pub(super) const DEFAULT_GAP_MS: u32 = 40;

pub(super) struct Frame {
    pub rgba: Arc<[u8]>,
    /// Milliseconds before the next frame; 0 is a gapless frame, skipped
    /// while running.
    pub gap: u32,
    /// Changes with every edit, so the UI converts the texture again.
    pub serial: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Run {
    #[default]
    Stopped,
    /// Runs, but waits at the last frame for more frames.
    Loading,
    /// Runs and loops.
    Running,
}

pub(super) struct Animation {
    /// `frames[0]` is the root frame.
    pub frames: Vec<Frame>,
    /// Index of the frame shown.
    pub current: usize,
    pub run: Run,
    /// Loops to play before stopping; 0 is forever.
    pub max_loops: u32,
    pub current_loop: u32,
    /// When the current frame was shown.
    pub shown_at: Instant,
}

impl Animation {
    /// An image with only its root frame (gap 0, as in kitty).
    pub fn new(root: Arc<[u8]>, serial: u64) -> Self {
        Self {
            frames: vec![Frame {
                rgba: root,
                gap: 0,
                serial,
            }],
            current: 0,
            run: Run::Stopped,
            max_loops: 0,
            current_loop: 0,
            shown_at: Instant::now(),
        }
    }

    fn animatable(&self) -> bool {
        self.run != Run::Stopped
            && self.frames.len() > 1
            && self.frames.iter().any(|f| f.gap > 0)
            && (self.max_loops == 0 || self.current_loop < self.max_loops)
    }

    /// Advances to the next shown frame if the current one's gap is over.
    /// Returns whether the frame changed, and when the next change is due
    /// (`None` when the animation doesn't run or waits for frames).
    pub fn tick(&mut self, now: Instant) -> (bool, Option<Instant>) {
        if !self.animatable() {
            return (false, None);
        }
        let due = |a: &Self| a.shown_at + Duration::from_millis(a.frames[a.current].gap.into());
        if now < due(self) {
            return (false, Some(due(self)));
        }
        // Skip gapless frames; at the end, loop or wait (kitty's rules).
        let mut next = self.current;
        loop {
            next = (next + 1) % self.frames.len();
            if next == 0 {
                if self.run == Run::Loading {
                    return (false, None);
                }
                self.current_loop += 1;
                if self.max_loops != 0 && self.current_loop >= self.max_loops {
                    return (false, None);
                }
            }
            if self.frames[next].gap != 0 {
                break;
            }
        }
        self.current = next;
        self.shown_at = now;
        (true, Some(due(self)))
    }
}

/// A canvas of `width` × `height` pixels filled with an RGBA color given as
/// `0xRRGGBBAA` (the `Y` key).
pub(super) fn canvas(width: u32, height: u32, color: u32) -> Vec<u8> {
    color.to_be_bytes().repeat(width as usize * height as usize)
}

/// A rectangle of pixels to copy: `w` × `h` from (`src_x`, `src_y`) in the
/// source to (`dst_x`, `dst_y`) in the destination.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rect {
    pub src_x: u32,
    pub src_y: u32,
    pub dst_x: u32,
    pub dst_y: u32,
    pub w: u32,
    pub h: u32,
}

/// Composes a rectangle of `src` (an image `src_width` wide) onto `dst` (an
/// image `dst_width` × `dst_height`), alpha blended or replacing pixels. The
/// part outside the destination is dropped.
pub(super) fn compose(
    dst: &mut [u8],
    dst_width: u32,
    dst_height: u32,
    src: &[u8],
    src_width: u32,
    r: Rect,
    blend: bool,
) {
    let w = r.w.min(dst_width.saturating_sub(r.dst_x)) as usize;
    let h = r.h.min(dst_height.saturating_sub(r.dst_y));
    for row in 0..h {
        let s = ((r.src_y + row) * src_width + r.src_x) as usize * 4;
        let d = ((r.dst_y + row) * dst_width + r.dst_x) as usize * 4;
        let (Some(s), Some(d)) = (src.get(s..s + w * 4), dst.get_mut(d..d + w * 4)) else {
            break;
        };
        if blend {
            for (d, s) in d
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(s.as_chunks::<4>().0)
            {
                blend_pixel(d, s);
            }
        } else {
            d.copy_from_slice(s);
        }
    }
}

/// `src` over `dst`, straight (not premultiplied) alpha.
fn blend_pixel(dst: &mut [u8; 4], src: &[u8; 4]) {
    let sa = u32::from(src[3]);
    if sa == 255 {
        *dst = *src;
        return;
    }
    if sa == 0 {
        return;
    }
    let da = u32::from(dst[3]);
    // Output alpha, scaled by 255.
    let oa = sa * 255 + da * (255 - sa);
    for c in 0..3 {
        let v = u32::from(src[c]) * sa * 255 + u32::from(dst[c]) * da * (255 - sa);
        dst[c] = ((v + oa / 2) / oa) as u8;
    }
    dst[3] = ((oa + 127) / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(gap: u32) -> Frame {
        Frame {
            rgba: Arc::from(&[0u8; 4][..]),
            gap,
            serial: 0,
        }
    }

    fn anim(gaps: &[u32], run: Run) -> (Animation, Instant) {
        let start = Instant::now();
        let mut a = Animation::new(Arc::from(&[0u8; 4][..]), 0);
        a.frames = gaps.iter().map(|&g| frame(g)).collect();
        a.run = run;
        a.shown_at = start;
        (a, start)
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn runs_frame_by_frame_and_loops() {
        let (mut a, t) = anim(&[100, 50, 70], Run::Running);
        assert_eq!(a.tick(t + MS(99)), (false, Some(t + MS(100))));
        assert_eq!(a.tick(t + MS(100)), (true, Some(t + MS(150))));
        assert_eq!(a.current, 1);
        // Late: one frame per tick, timed from when it is shown.
        assert_eq!(a.tick(t + MS(400)), (true, Some(t + MS(470))));
        assert_eq!(a.current, 2);
        assert!(a.tick(t + MS(470)).0);
        assert_eq!((a.current, a.current_loop), (0, 1));
    }

    #[test]
    fn gapless_frames_are_skipped() {
        // The root frame has no gap by default: it is never shown.
        let (mut a, t) = anim(&[0, 30, 0, 20], Run::Running);
        a.tick(t);
        assert_eq!(a.current, 1);
        a.tick(t + MS(30));
        assert_eq!(a.current, 3);
        a.tick(t + MS(50));
        assert_eq!(a.current, 1);
    }

    #[test]
    fn stops_after_its_loops_and_waits_while_loading() {
        // v=3: two loops.
        let (mut a, t) = anim(&[10, 10], Run::Running);
        a.max_loops = 2;
        let mut now = t;
        for _ in 0..3 {
            now += MS(10);
            assert!(a.tick(now).0);
        }
        now += MS(10);
        assert_eq!(a.tick(now), (false, None));
        assert_eq!((a.current, a.current_loop), (1, 2));

        let (mut a, t) = anim(&[10, 10], Run::Loading);
        assert!(a.tick(t + MS(10)).0);
        assert_eq!(a.tick(t + MS(100)), (false, None));
        a.frames.push(frame(10));
        assert!(a.tick(t + MS(100)).0);
        assert_eq!(a.current, 2);
    }

    #[test]
    fn stopped_or_single_frame_does_not_animate() {
        let (mut a, t) = anim(&[10, 10], Run::Stopped);
        assert_eq!(a.tick(t + MS(50)), (false, None));
        let (mut a, t) = anim(&[10], Run::Running);
        assert_eq!(a.tick(t + MS(50)), (false, None));
    }

    #[test]
    fn compose_blends_or_replaces_and_clips() {
        // 2×2 opaque red, compose a 2×1 half-transparent green row at (1, 1).
        let mut dst = canvas(2, 2, 0xff0000ff);
        let src = [0, 255, 0, 128, 0, 255, 0, 128];
        let rect = Rect {
            src_x: 0,
            src_y: 0,
            dst_x: 1,
            dst_y: 1,
            w: 2,
            h: 1,
        };
        compose(&mut dst, 2, 2, &src, 2, rect, true);
        assert_eq!(&dst[..12], &canvas(3, 1, 0xff0000ff)[..]);
        assert_eq!(&dst[12..], &[127, 128, 0, 255]);
        compose(&mut dst, 2, 2, &src, 2, rect, false);
        assert_eq!(&dst[12..], &[0, 255, 0, 128]);
    }

    #[test]
    fn blending_onto_transparent_keeps_the_color() {
        let mut px = [0, 0, 0, 0];
        blend_pixel(&mut px, &[10, 20, 30, 40]);
        assert_eq!(px, [10, 20, 30, 40]);
    }
}
