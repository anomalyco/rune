//! The display's refresh grid, from DWM: when presentations happen. A video picks its picture
//! for the refresh a frame will show at, rather than the moment the frame was built, so the
//! wobble in when frames are built never shows a picture twice or skips one.

use crate::time::Instant;
use std::sync::Mutex;
use std::time::Duration;
use windows_sys::Win32::Graphics::Dwm::DWM_TIMING_INFO;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// How long a reading of the grid is trusted before it's read again.
const FRESH: Duration = Duration::from_secs(1);
/// A frame built now presents at the first refresh at least this far ahead.
const BUILD: Duration = Duration::from_millis(2);
const DEFAULT_PERIOD: Duration = Duration::from_nanos(16_666_667);

type GetTiming = unsafe extern "system" fn(isize, *mut DWM_TIMING_INFO) -> i32;

#[derive(Clone, Copy)]
struct Grid {
    read: Instant,
    vblank: Instant,
    period: Duration,
}

static GRID: Mutex<Option<Grid>> = Mutex::new(None);

fn grid() -> Option<Grid> {
    let now = Instant::now();
    let mut slot = GRID.lock().unwrap();
    if let Some(g) = *slot
        && now.saturating_duration_since(g.read) < FRESH
    {
        return Some(g);
    }
    let fresh = read(now);
    *slot = fresh.or(*slot);
    *slot
}

fn read(now: Instant) -> Option<Grid> {
    static TIMING: std::sync::OnceLock<Option<GetTiming>> = std::sync::OnceLock::new();
    // SAFETY: dwmapi's documented entry point at its documented signature.
    let get = (*TIMING.get_or_init(|| unsafe { super::mf::symbol(super::mf::system_library("dwmapi.dll")?, c"DwmGetCompositionTimingInfo") }))?;
    let mut info: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
    let (mut qpc, mut frequency) = (0i64, 0i64);
    // SAFETY: plain reads into locals.
    unsafe {
        if get(0, &mut info) < 0 {
            return None;
        }
        QueryPerformanceCounter(&mut qpc);
        QueryPerformanceFrequency(&mut frequency);
    }
    if frequency <= 0 || info.qpcRefreshPeriod == 0 {
        return None;
    }
    let ticks = |t: i64| Duration::from_nanos((t as i128 * 1_000_000_000 / frequency as i128) as u64);
    let period = ticks(info.qpcRefreshPeriod as i64);
    let ago = qpc - info.qpcVBlank as i64;
    let vblank = if ago >= 0 { now - ticks(ago) } else { now + ticks(-ago) };
    (Duration::from_micros(2_000)..Duration::from_millis(100)).contains(&period).then_some(Grid { read: now, vblank, period })
}

/// The display's refresh period.
pub(super) fn period() -> Duration {
    grid().map_or(DEFAULT_PERIOD, |g| g.period)
}

/// The refresh a frame built at `t` presents at: the first at least `BUILD` after it.
pub(super) fn snap(t: Instant) -> Instant {
    let Some(g) = grid() else { return t };
    let t = t + BUILD;
    let since = t.saturating_duration_since(g.vblank).as_nanos();
    let period = g.period.as_nanos().max(1);
    let k = since.div_ceil(period);
    g.vblank + Duration::from_nanos((k * period) as u64)
}
