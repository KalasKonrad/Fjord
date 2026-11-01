// ── fjord-app · playback/hitch.rs ─────────────────────────────────────────
//   HitchWatch   notices brief choppiness mid-playback, for the log only: a video frame that came
//                late for the file's frame rate, or a jump in mpv's dropped/mistimed/delayed frame
//                counts. Close events merge into one Hitch (QUIET_FOR apart, at most MAX_LASTING);
//                pure, unit-tested — times are passed in. Seeks, pauses and the first seconds
//                of a file don't count (Sample.active + SETTLE_FOR)
//   Hitch        one such moment; its line says where the time went: `slowest frame` = Fjord's own
//                frame (mpv's render + Slint's draw), `UI thread late` = the 16 ms tick ran late
//                (that frame or other work on the UI thread). Both small = the frame didn't
//                come from Fjord's side being busy: mpv had none yet, or the compositor held it
// ─────────────────────────────────────────────────────────────────────────────
use std::time::{Duration, Instant};

/// A frame this many frame periods after the previous one counts as late.
const LATE_FACTOR: f64 = 1.5;
/// A hitch has ended after this long without a late frame or a counter jump.
const QUIET_FOR: Duration = Duration::from_secs(2);
/// A longer one is reported in parts, so the log says something while it lasts.
const MAX_LASTING: Duration = Duration::from_secs(30);
/// After a seek, a pause or the start of a file frames are irregular for a moment.
const SETTLE_FOR: Duration = Duration::from_secs(1);
/// How long a late tick still counts for a late frame that comes after it.
const RECENT_FOR: Duration = Duration::from_millis(500);
/// The tick's own interval (wire_mpv_timer).
const TICK: Duration = Duration::from_millis(16);
/// Lines per player; the 5-minute stats line keeps counting.
const MAX_LOGGED: u32 = 100;

/// mpv's frame counters for this file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FrameCounts {
    pub drops: i64,
    pub decoder_drops: i64,
    pub mistimed: i64,
    pub delayed: i64,
}

impl FrameCounts {
    fn since(self, before: Self) -> Self {
        Self {
            drops: (self.drops - before.drops).max(0),
            decoder_drops: (self.decoder_drops - before.decoder_drops).max(0),
            mistimed: (self.mistimed - before.mistimed).max(0),
            delayed: (self.delayed - before.delayed).max(0),
        }
    }

    fn any(self) -> bool {
        self != Self::default()
    }

    fn add(&mut self, other: Self) {
        self.drops += other.drops;
        self.decoder_drops += other.decoder_drops;
        self.mistimed += other.mistimed;
        self.delayed += other.delayed;
    }
}

/// What the tick reports to the watch.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    /// Video actually playing: not paused, not seeking, some seconds into the file.
    pub active: bool,
    pub fps: f64,
    pub counts: FrameCounts,
    pub cache_secs: f64,
    pub buffering: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hitch {
    pub lasted: Duration,
    pub late_frames: u32,
    pub worst_gap: Duration,
    pub slowest_frame: Duration,
    pub ui_late: Duration,
    pub counts: FrameCounts,
    pub lowest_cache_secs: f64,
    pub buffering: bool,
}

impl Hitch {
    pub(crate) fn line(&self, pos: f64, fps: f64) -> String {
        let c = self.counts;
        format!(
            "video hitch before {pos:.0}s: {} ms, {} late frame(s) at {fps:.2} fps, worst gap {} ms; \
             slowest frame {} ms, UI thread late {} ms; mpv drops +{} decoder +{} mistimed +{} \
             delayed +{}; lowest cache {:.1}s{}",
            self.lasted.as_millis(),
            self.late_frames,
            self.worst_gap.as_millis(),
            self.slowest_frame.as_millis(),
            self.ui_late.as_millis(),
            c.drops,
            c.decoder_drops,
            c.mistimed,
            c.delayed,
            self.lowest_cache_secs,
            if self.buffering { ", buffering" } else { "" },
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct Open {
    first: Instant,
    last: Instant,
    hitch: Hitch,
}

#[derive(Debug, Default)]
pub(crate) struct HitchWatch {
    armed_at: Option<Instant>,
    period: Option<Duration>,
    last_frame: Option<Instant>,
    last_tick: Option<Instant>,
    counts: Option<FrameCounts>,
    last_frame_took: Duration,
    recent_ui_late: Option<(Instant, Duration)>,
    cache_secs: f64,
    open: Option<Open>,
    /// Hitches this player so far, logged or not.
    pub total: u32,
}

impl HitchWatch {
    fn armed(&self, now: Instant) -> bool {
        self.armed_at.is_some_and(|at| now >= at)
    }

    fn touch(&mut self, now: Instant, gap: Duration) -> &mut Open {
        let (took, cache) = (self.last_frame_took, self.cache_secs);
        let ui_late = match self.recent_ui_late {
            Some((at, late)) if now.duration_since(at) <= RECENT_FOR => late,
            _ => Duration::ZERO,
        };
        let open = self.open.get_or_insert_with(|| Open {
            first: now.checked_sub(gap).unwrap_or(now),
            last: now,
            hitch: Hitch {
                lasted: Duration::ZERO,
                late_frames: 0,
                worst_gap: Duration::ZERO,
                slowest_frame: took,
                ui_late,
                counts: FrameCounts::default(),
                lowest_cache_secs: cache,
                buffering: false,
            },
        });
        open.last = now;
        open
    }

    /// A video frame was just rendered.
    pub(crate) fn on_frame(&mut self, now: Instant) {
        let previous = self.last_frame.replace(now);
        if !self.armed(now) {
            return;
        }
        if let (Some(previous), Some(period)) = (previous, self.period) {
            let gap = now.duration_since(previous);
            if gap >= period.mul_f64(LATE_FACTOR) {
                let hitch = &mut self.touch(now, gap).hitch;
                hitch.late_frames += 1;
                hitch.worst_gap = hitch.worst_gap.max(gap);
            }
        }
    }

    /// That frame's whole time on the UI thread.
    pub(crate) fn on_frame_done(&mut self, took: Duration) {
        self.last_frame_took = took;
        if let Some(open) = self.open.as_mut() {
            open.hitch.slowest_frame = open.hitch.slowest_frame.max(took);
        }
    }

    /// The 16 ms tick; a finished hitch to log (None past MAX_LOGGED, `total` still counts it).
    pub(crate) fn on_tick(&mut self, now: Instant, sample: Sample) -> Option<Hitch> {
        let late = self.last_tick.replace(now).map_or(Duration::ZERO, |t| {
            now.duration_since(t).saturating_sub(TICK)
        });
        let keep = self
            .recent_ui_late
            .is_some_and(|(at, worst)| now.duration_since(at) <= RECENT_FOR && worst >= late);
        if !keep {
            self.recent_ui_late = Some((now, late));
        }
        self.cache_secs = sample.cache_secs;
        self.period = (sample.fps > 0.0).then(|| Duration::from_secs_f64(1.0 / sample.fps));

        let jump = sample
            .counts
            .since(self.counts.replace(sample.counts).unwrap_or(sample.counts));
        if !sample.active {
            self.armed_at = None;
            self.last_frame = None;
            return self.finish();
        }
        let armed_at = *self.armed_at.get_or_insert(now + SETTLE_FOR);
        if now >= armed_at && jump.any() {
            self.touch(now, Duration::ZERO).hitch.counts.add(jump);
        }
        let open = self.open.as_mut()?;
        open.hitch.ui_late = open.hitch.ui_late.max(late);
        open.hitch.lowest_cache_secs = open.hitch.lowest_cache_secs.min(sample.cache_secs);
        open.hitch.buffering |= sample.buffering;
        let over = now.duration_since(open.last) >= QUIET_FOR
            || now.duration_since(open.first) >= MAX_LASTING;
        if over { self.finish() } else { None }
    }

    fn finish(&mut self) -> Option<Hitch> {
        let open = self.open.take()?;
        self.total += 1;
        (self.total <= MAX_LOGGED).then_some(Hitch {
            lasted: open.last.duration_since(open.first),
            ..open.hitch
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f64 = 24.0;
    const FRAME: Duration = Duration::from_millis(42);

    fn sample(active: bool, counts: FrameCounts) -> Sample {
        Sample {
            active,
            fps: FPS,
            counts,
            cache_secs: 60.0,
            buffering: false,
        }
    }

    /// A watch that has settled, with a regular frame just drawn at the returned time.
    fn settled() -> (HitchWatch, Instant) {
        let mut w = HitchWatch::default();
        let mut t = Instant::now();
        let end = t + SETTLE_FOR;
        while t < end {
            assert_eq!(tick(&mut w, t), None);
            t += TICK;
        }
        assert_eq!(tick(&mut w, t), None);
        w.on_frame(t);
        (w, t)
    }

    fn tick(w: &mut HitchWatch, at: Instant) -> Option<Hitch> {
        w.on_tick(at, sample(true, FrameCounts::default()))
    }

    #[test]
    fn regular_frames_are_no_hitch() {
        let (mut w, mut t) = settled();
        for _ in 0..200 {
            t += FRAME;
            w.on_frame(t);
            assert_eq!(tick(&mut w, t), None);
        }
        assert_eq!(tick(&mut w, t + QUIET_FOR), None);
        assert_eq!(w.total, 0);
    }

    #[test]
    fn late_frames_close_together_are_one_hitch() {
        let (mut w, t) = settled();
        let t1 = t + Duration::from_millis(120);
        w.on_frame(t1);
        let t2 = t1 + Duration::from_millis(90);
        w.on_frame(t2);
        assert_eq!(tick(&mut w, t2 + QUIET_FOR - TICK), None);
        let h = tick(&mut w, t2 + QUIET_FOR).unwrap();
        assert_eq!(h.late_frames, 2);
        assert_eq!(h.worst_gap, Duration::from_millis(120));
        assert_eq!(h.lasted, Duration::from_millis(210));
        assert_eq!(w.total, 1);
    }

    #[test]
    fn it_says_how_busy_fjord_was_just_before_the_late_frame() {
        let (mut w, t) = settled();
        w.on_frame_done(Duration::from_millis(95));
        let t1 = t + Duration::from_millis(116);
        assert_eq!(tick(&mut w, t1), None);
        w.on_frame(t1);
        let mut t2 = t1;
        let h = loop {
            t2 += TICK;
            if let Some(h) = tick(&mut w, t2) {
                break h;
            }
        };
        assert_eq!(h.slowest_frame, Duration::from_millis(95));
        assert_eq!(h.ui_late, Duration::from_millis(100));
    }

    #[test]
    fn a_jump_in_mpvs_counts_is_a_hitch_without_a_late_frame() {
        let (mut w, t) = settled();
        let counts = FrameCounts {
            mistimed: 3,
            ..FrameCounts::default()
        };
        assert_eq!(w.on_tick(t + FRAME, sample(true, counts)), None);
        let h = w
            .on_tick(t + FRAME + QUIET_FOR, sample(true, counts))
            .unwrap();
        assert_eq!(h.counts, counts);
        assert_eq!(h.late_frames, 0);
    }

    #[test]
    fn seeks_and_pauses_and_settling_dont_count() {
        let (mut w, t) = settled();
        // A count that rose during the seek and a frame long after it.
        let counts = FrameCounts {
            drops: 5,
            ..FrameCounts::default()
        };
        let t1 = t + Duration::from_secs(4);
        assert_eq!(w.on_tick(t1, sample(false, counts)), None);
        w.on_frame(t1 + FRAME);
        // Active again: the first second's irregular frames are ignored too.
        let t2 = t1 + FRAME;
        assert_eq!(w.on_tick(t2, sample(true, counts)), None);
        w.on_frame(t2 + Duration::from_millis(300));
        assert_eq!(w.on_tick(t2 + QUIET_FOR, sample(true, counts)), None);
        assert_eq!(w.total, 0);
    }

    #[test]
    fn a_hitch_cut_off_by_a_pause_is_still_reported() {
        let (mut w, t) = settled();
        let t1 = t + Duration::from_millis(200);
        w.on_frame(t1);
        let h = w.on_tick(t1 + FRAME, sample(false, FrameCounts::default()));
        assert_eq!(h.unwrap().late_frames, 1);
    }

    #[test]
    fn a_long_one_is_reported_in_parts_and_the_log_is_capped() {
        let (mut w, mut t) = settled();
        let mut lines = 0;
        for _ in 0..(MAX_LOGGED + 20) {
            let end = t + MAX_LASTING;
            while t < end {
                t += Duration::from_millis(100);
                w.on_frame(t);
                if tick(&mut w, t).is_some() {
                    lines += 1;
                }
            }
        }
        assert_eq!(lines, MAX_LOGGED);
        assert!(w.total > MAX_LOGGED);
    }

    #[test]
    fn the_line_names_the_numbers() {
        let h = Hitch {
            lasted: Duration::from_millis(250),
            late_frames: 3,
            worst_gap: Duration::from_millis(120),
            slowest_frame: Duration::from_millis(8),
            ui_late: Duration::from_millis(4),
            counts: FrameCounts {
                delayed: 2,
                ..FrameCounts::default()
            },
            lowest_cache_secs: 41.5,
            buffering: false,
        };
        let line = h.line(1234.4, 23.976);
        assert!(line.starts_with("video hitch before 1234s: 250 ms, 3 late frame(s) at 23.98 fps"));
        assert!(line.contains("slowest frame 8 ms, UI thread late 4 ms"));
        assert!(line.contains("delayed +2; lowest cache 41.5s"));
    }
}
