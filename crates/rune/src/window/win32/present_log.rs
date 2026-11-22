//! `RUNE_PRESENT_LOG=<file>`: one JSON line per present of a window's swap chain, with DXGI's
//! frame statistics (which refresh showed which present, and whether the compositor composed it
//! or the display scanned it out directly), and one per signal of the swap chain's waitable
//! object. Times are raw `QueryPerformanceCounter` ticks, the clock DXGI reports in. For measuring
//! the present path from inside the process, without ETW; off, it costs one cached flag.

#![allow(unsafe_code)]

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Dxgi::{DXGI_FRAME_STATISTICS, DXGI_FRAME_STATISTICS_MEDIA, IDXGISwapChainMedia};
use windows_core::Interface;

static LOG: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();

fn log() -> Option<&'static Mutex<std::io::BufWriter<std::fs::File>>> {
    LOG.get_or_init(|| {
        let path = std::env::var_os("RUNE_PRESENT_LOG")?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        let log = Mutex::new(std::io::BufWriter::new(file));
        write(&log, &format!("{{\"kind\":\"start\",\"qpc_hz\":{},\"dwm\":{}}}", qpc_hz(), dwm()));
        Some(log)
    })
    .as_ref()
}

fn write(log: &Mutex<std::io::BufWriter<std::fs::File>>, line: &str) {
    let mut w = log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    writeln!(w, "{line}").ok();
    w.flush().ok();
}

fn qpc() -> i64 {
    let mut t = 0;
    // SAFETY: writes one i64.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
    t
}

fn qpc_hz() -> i64 {
    let mut f = 0;
    // SAFETY: writes one i64.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut f) };
    f
}

/// The compositor's refresh and composition rates and its refresh period, as a JSON object.
fn dwm() -> String {
    use windows_sys::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
    // SAFETY: a zeroed plain struct whose size field is set; a null window asks for the whole desktop.
    let mut info: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
    let hr = unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut info) };
    if hr != 0 {
        return format!("{{\"hr\":{hr}}}");
    }
    // The struct is packed: its fields are copied out, never borrowed.
    let (refresh, compose) = (info.rateRefresh, info.rateCompose);
    let (period, vblank, count, dropped, missed) = (info.qpcRefreshPeriod, info.qpcVBlank, info.cRefresh, info.cFramesDropped, info.cFramesMissed);
    let (rn, rd, cn, cd) = (refresh.uiNumerator, refresh.uiDenominator, compose.uiNumerator, compose.uiDenominator);
    format!(
        "{{\"refresh\":[{rn},{rd}],\"compose\":[{cn},{cd}],\"qpc_refresh_period\":{period},\"qpc_vblank\":{vblank},\"refresh_count\":{count},\"frames_dropped\":{dropped},\"frames_missed\":{missed}}}"
    )
}

/// After `surface`'s present: its count and the frame statistics DXGI has so far.
pub(crate) fn presented(surface: &wgpu::Surface) {
    let Some(log) = log() else { return };
    let at = qpc();
    // SAFETY: reads the live swap chain's statistics; nothing is kept.
    let Some(hal) = (unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }) else { return };
    let buffers = hal.buffer_count().unwrap_or(0);
    let Some(chain) = hal.swap_chain() else { return };
    let last = unsafe { chain.GetLastPresentCount() }.unwrap_or(0);
    let mut media = DXGI_FRAME_STATISTICS_MEDIA::default();
    let line = match chain.cast::<IDXGISwapChainMedia>().ok().map(|m| unsafe { m.GetFrameStatisticsMedia(&mut media) }) {
        Some(Ok(())) => format!(
            "{{\"kind\":\"present\",\"qpc\":{at},\"buffers\":{buffers},\"last_present\":{last},\"present\":{},\"present_refresh\":{},\"sync_refresh\":{},\"sync_qpc\":{},\"mode\":{},\"approved\":{}}}",
            media.PresentCount, media.PresentRefreshCount, media.SyncRefreshCount, media.SyncQPCTime, media.CompositionMode.0, media.ApprovedPresentDuration
        ),
        media_err => {
            let mut stats = DXGI_FRAME_STATISTICS::default();
            match unsafe { chain.GetFrameStatistics(&mut stats) } {
                Ok(()) => format!(
                    "{{\"kind\":\"present\",\"qpc\":{at},\"buffers\":{buffers},\"last_present\":{last},\"present\":{},\"present_refresh\":{},\"sync_refresh\":{},\"sync_qpc\":{},\"media_err\":{}}}",
                    stats.PresentCount, stats.PresentRefreshCount, stats.SyncRefreshCount, stats.SyncQPCTime,
                    media_err.and_then(|r| r.err()).map_or(0, |e| e.code().0)
                ),
                Err(e) => format!("{{\"kind\":\"present\",\"qpc\":{at},\"buffers\":{buffers},\"last_present\":{last},\"err\":{}}}", e.code().0),
            }
        }
    };
    write(log, &line);
}

/// The swap chain's waitable object was found signalled: `how` is `"wait"` (the registered
/// thread-pool wait came back) or `"poll"` (the next frame found room at once).
pub(crate) fn room(how: &str) {
    let Some(log) = log() else { return };
    write(log, &format!("{{\"kind\":\"room\",\"qpc\":{},\"how\":\"{how}\"}}", qpc()));
}

/// Once a second while presenting: the compositor's counters.
pub(crate) fn dwm_tick() {
    static LAST: Mutex<i64> = Mutex::new(0);
    let Some(log) = log() else { return };
    let now = qpc();
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if now - *last < qpc_hz() {
        return;
    }
    *last = now;
    write(log, &format!("{{\"kind\":\"dwm\",\"qpc\":{now},\"dwm\":{}}}", dwm()));
}
