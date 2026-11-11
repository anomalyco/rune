//! The idle meters: a `poll` that says it changed what shows when nothing did, and a window that
//! keeps drawing the same frame while nothing asks it to. "Nothing changed" is the test the
//! native host uses to skip a present: the same instance bytes and surface, with nothing drawn
//! outside the instances. Only frames with no input and no motion are compared, so a busy
//! frame costs one flag and release builds hold nothing at all.

/// Quiet frames in a row `poll` may say it changed something before nothing changing warns.
#[cfg(debug_assertions)]
const POLL_FRAMES: u32 = 30;
/// Self-woken frames that draw nothing new, and within how many seconds, before a window warns.
#[cfg(debug_assertions)]
const IDLE_FRAMES: u32 = 120;
#[cfg(debug_assertions)]
const IDLE_SECS: f64 = 2.0;

/// One window's idle meters.
#[derive(Default)]
pub(crate) struct IdleMeter {
    #[cfg(debug_assertions)]
    meter: Meter,
}

/// What a frame drew, as far as telling the next one apart goes.
#[cfg(debug_assertions)]
pub(crate) struct Drawn<'a> {
    pub instances: &'a [u8],
    /// The surface it drew into and the image copies so far, hashed.
    pub key: u64,
    /// It drew something the instances don't hold (`SurfaceDraw`, native materials, video), so
    /// the same instances may not mean the same pixels.
    pub opaque: bool,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
struct Start { quiet: bool, timer: bool, live: bool }

#[cfg(debug_assertions)]
#[derive(Default)]
struct Meter {
    frame: Option<Start>,
    /// `poll` returned true since the last frame ended.
    polled: bool,
    /// The last quiet frame's instances and key; none after a frame with input or motion.
    last: Option<(Vec<u8>, u64)>,
    poll_streak: u32,
    idle: Streak,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
enum Woke { Live { polled: bool }, Timer, TimerLive, TimerPoll }

#[cfg(debug_assertions)]
impl Woke {
    fn label(self) -> &'static str {
        match self {
            Self::TimerPoll => "a `wake_after` loop whose `poll` returns true",
            Self::Timer => "a `wake_after` loop",
            Self::TimerLive => "a `wake_after` loop and a live value",
            Self::Live { .. } => "a live value",
        }
    }
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Default)]
enum Streak { #[default] Watching, Counting { from: f64, frames: u32, woke: Woke } }

#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
struct Reading { clock: f64, quiet: bool, animating: bool, same: Option<bool>, timer: bool, live: bool, polled: bool }

#[cfg(debug_assertions)]
#[inline]
fn step(streak: &mut Streak, r: Reading) -> crate::machine::Effects<(Woke, f64)> {
    use crate::machine::Effects;
    if !r.quiet || r.animating || r.same == Some(false) {
        *streak = Streak::Watching;
        return Effects::none();
    }
    if r.same.is_none() || !(r.timer || r.live) { return Effects::none(); }
    let (from, frames, previous) = match *streak {
        Streak::Counting { from, frames, woke } if !(r.clock - from > IDLE_SECS) => (from, frames, Some(woke)),
        _ => (r.clock, 0, None),
    };
    let timer = r.timer || matches!(previous, Some(Woke::Timer | Woke::TimerLive | Woke::TimerPoll));
    let live = r.live || matches!(previous, Some(Woke::Live { .. } | Woke::TimerLive));
    let polled = r.polled || matches!(previous, Some(Woke::Live { polled: true } | Woke::TimerPoll));
    let woke = match (timer, live, polled) {
        (true, _, true) => Woke::TimerPoll,
        (true, false, false) => Woke::Timer,
        (true, true, false) => Woke::TimerLive,
        (false, ..) => Woke::Live { polled },
    };
    let frames = frames + 1;
    *streak = Streak::Counting { from, frames, woke };
    if frames == IDLE_FRAMES { Effects::one((woke, r.clock - from)) } else { Effects::none() }
}

#[cfg(all(test, debug_assertions))]
#[path = "idle_tests.rs"]
mod tests;

impl IdleMeter {
    /// Passes `changed`, `poll`'s answer, through.
    #[inline(always)]
    pub(crate) fn polled(&mut self, changed: bool) -> bool {
        #[cfg(debug_assertions)]
        { self.meter.polled |= changed; }
        changed
    }

    /// A frame starts. `quiet`: no input waits and nothing moves; `timer`: a due `wake_after`
    /// woke it; `live`: a live value did.
    #[cfg(debug_assertions)]
    pub(crate) fn begins(&mut self, quiet: bool, timer: bool, live: bool) {
        self.meter.frame = Some(Start { quiet, timer, live });
    }

    /// The frame drew `drawn` at `clock` seconds, leaving motion running if `animating`. `name`
    /// names the window, when it has a title.
    #[cfg(debug_assertions)]
    pub(crate) fn ends(&mut self, clock: f64, animating: bool, drawn: Drawn, name: impl Fn() -> Option<String>) {
        let m = &mut self.meter;
        let Some(start) = m.frame.take() else { return };
        let polled = std::mem::take(&mut m.polled);
        let quiet = start.quiet && !drawn.opaque;
        // Whether it drew what the last frame did: unknown unless that one was quiet too.
        let same = quiet.then(|| m.last.as_ref().map(|(bytes, key)| *key == drawn.key && bytes[..] == *drawn.instances)).flatten();
        if quiet {
            let (bytes, key) = m.last.get_or_insert_default();
            bytes.clear();
            bytes.extend_from_slice(drawn.instances);
            *key = drawn.key;
        } else {
            m.last = None;
        }
        let same = same.map(|same| same && !animating);
        let site = || name().map_or_else(String::new, |name| format!(" {name:?}"));
        if polled && let Some(same) = same {
            m.poll_streak = if same { m.poll_streak + 1 } else { 0 };
            if m.poll_streak == POLL_FRAMES {
                let window = site();
                super::warn_once(format!("idle-poll{window}"), || format!(
                    "`poll`{} returned true but nothing changed ({POLL_FRAMES} frames in a row); return \
                     true only when visible state changed", if window.is_empty() { String::new() } else { format!(" of window{window}") }
                ));
            }
        }
        let reading = Reading { clock, quiet, animating, same, timer: start.timer, live: start.live, polled };
        if let crate::machine::Effects::One((woke, secs)) = step(&mut m.idle, reading) {
            let window = site();
            let by = woke.label();
            super::warn_once(format!("idle-draw{window}"), || format!(
                "idle window{window} still drawing: check `wake_after` loops ({IDLE_FRAMES} frames in {secs:.1} s \
                 with no input, motion or visible change, woken by {by})"
            ));
        }
    }
}
