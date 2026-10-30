//! libmpv for windowed playback; headless stills and scrub previews use the same handle.
//!
//! Live clips use `vo=libmpv` with the software render API (BGRA into [`NativeSurface`]) and
//! libmpv's audio output. One handle, one clock.
use super::*;
use super::linux_still::fit_bgra;
use libmpv2_sys as sys;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::time::Duration;

const AUDIO_TICK: Duration = Duration::from_millis(50);
const SEEK_PATIENCE: Duration = Duration::from_secs(2);

fn mpv_err(code: c_int) -> Result<(), String> {
    if code >= 0 {
        Ok(())
    } else {
        let msg = unsafe { CStr::from_ptr(sys::mpv_error_string(code)) };
        Err(msg.to_string_lossy().into_owned())
    }
}

fn source_path(source: &MediaSource) -> Result<String, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            Ok(std::path::absolute(path).unwrap_or_else(|_| path.clone()).to_string_lossy().into_owned())
        }
        MediaSource::Url(url) => Ok(url.clone()),
    }
}

fn set_option(handle: *mut sys::mpv_handle, name: &str, value: &str) -> Result<(), String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    let value = CString::new(value).map_err(|e| e.to_string())?;
    mpv_err(unsafe { sys::mpv_set_option_string(handle, name.as_ptr(), value.as_ptr()) })
}

fn set_prop_string(handle: *mut sys::mpv_handle, name: &str, value: &str) -> Result<(), String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    let value = CString::new(value).map_err(|e| e.to_string())?;
    mpv_err(unsafe { sys::mpv_set_property_string(handle, name.as_ptr(), value.as_ptr()) })
}

fn set_prop_flag(handle: *mut sys::mpv_handle, name: &str, value: bool) -> Result<(), String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    let mut v: i64 = if value { 1 } else { 0 };
    mpv_err(unsafe {
        sys::mpv_set_property(handle, name.as_ptr(), sys::mpv_format_MPV_FORMAT_FLAG, &mut v as *mut _ as *mut c_void)
    })
}

fn set_prop_f64(handle: *mut sys::mpv_handle, name: &str, value: f64) -> Result<(), String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    let mut v = value;
    mpv_err(unsafe {
        sys::mpv_set_property(handle, name.as_ptr(), sys::mpv_format_MPV_FORMAT_DOUBLE, &mut v as *mut _ as *mut c_void)
    })
}

fn get_prop_f64(handle: *mut sys::mpv_handle, name: &str) -> Option<f64> {
    let name = CString::new(name).ok()?;
    let mut v = 0.0f64;
    let err = unsafe {
        sys::mpv_get_property(handle, name.as_ptr(), sys::mpv_format_MPV_FORMAT_DOUBLE, &mut v as *mut _ as *mut c_void)
    };
    (err >= 0).then_some(v)
}

fn get_prop_flag(handle: *mut sys::mpv_handle, name: &str) -> Option<bool> {
    let name = CString::new(name).ok()?;
    let mut v: i64 = 0;
    let err = unsafe {
        sys::mpv_get_property(handle, name.as_ptr(), sys::mpv_format_MPV_FORMAT_FLAG, &mut v as *mut _ as *mut c_void)
    };
    (err >= 0).then_some(v != 0)
}

fn get_prop_i64(handle: *mut sys::mpv_handle, name: &str) -> Option<i64> {
    let name = CString::new(name).ok()?;
    let mut v: i64 = 0;
    let err = unsafe {
        sys::mpv_get_property(handle, name.as_ptr(), sys::mpv_format_MPV_FORMAT_INT64, &mut v as *mut _ as *mut c_void)
    };
    (err >= 0).then_some(v)
}

fn get_prop_string(handle: *mut sys::mpv_handle, name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    let ptr = unsafe { sys::mpv_get_property_string(handle, name.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    unsafe { sys::mpv_free(ptr as *mut c_void) };
    Some(s)
}

fn command(handle: *mut sys::mpv_handle, args: &[&str]) -> Result<(), String> {
    let cargs: Vec<CString> = args.iter().map(|a| CString::new(*a).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    let mut ptrs: Vec<*const c_char> = cargs.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(ptr::null());
    mpv_err(unsafe { sys::mpv_command(handle, ptrs.as_mut_ptr()) })
}

/// One libmpv handle: software-rendered BGRA frames and system audio.
struct Session {
    handle: *mut sys::mpv_handle,
    render: *mut sys::mpv_render_context,
    seeking: Option<(Instant, f64)>,
    await_frame: Option<Instant>,
    rate: f64,
    muted: bool,
    volume: f64,
    audio_track: Option<i32>,
    video_track: Option<i32>,
    subtitle: SubtitleSelect,
    kind: Kind,
    size: Option<(u32, u32)>,
}

// libmpv handles are used only under the Native mutex.
unsafe impl Send for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            if !self.render.is_null() {
                sys::mpv_render_context_free(self.render);
                self.render = ptr::null_mut();
            }
            if !self.handle.is_null() {
                sys::mpv_terminate_destroy(self.handle);
                self.handle = ptr::null_mut();
            }
        }
    }
}

impl Session {
    fn open(source: &MediaSource, kind: Kind, size: Option<(u32, u32)>) -> Result<Self, String> {
        let handle = unsafe { sys::mpv_create() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        let destroy_on_err = |handle: *mut sys::mpv_handle, e: String| -> Result<Self, String> {
            unsafe { sys::mpv_terminate_destroy(handle) };
            Err(e)
        };
        // Isolate from the user's mpv config; enable idle so the core stays up.
        if let Err(e) = set_option(handle, "config", "no") {
            return destroy_on_err(handle, e);
        }
        if let Err(e) = set_option(handle, "idle", "yes") {
            return destroy_on_err(handle, e);
        }
        if let Err(e) = set_option(handle, "osc", "no") {
            return destroy_on_err(handle, e);
        }
        if let Err(e) = set_option(handle, "input-default-bindings", "no") {
            return destroy_on_err(handle, e);
        }
        if let Err(e) = set_option(handle, "input-vo-keyboard", "no") {
            return destroy_on_err(handle, e);
        }
        match kind {
            Kind::Video => {
                // Software render API needs CPU frames; hw decoders often won't feed it.
                if let Err(e) = set_option(handle, "hwdec", "no") {
                    return destroy_on_err(handle, e);
                }
                if let Err(e) = set_option(handle, "vo", "libmpv") {
                    return destroy_on_err(handle, e);
                }
            }
            Kind::Audio => {
                if let Err(e) = set_option(handle, "video", "no") {
                    return destroy_on_err(handle, e);
                }
                if let Err(e) = set_option(handle, "vo", "null") {
                    return destroy_on_err(handle, e);
                }
            }
        }
        if let Err(e) = mpv_err(unsafe { sys::mpv_initialize(handle) }) {
            return destroy_on_err(handle, e);
        }
        let mut render = ptr::null_mut();
        if kind == Kind::Video {
            let api = sys::MPV_RENDER_API_TYPE_SW.as_ptr() as *const c_char;
            let mut params = [
                sys::mpv_render_param {
                    type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                    data: api as *mut c_void,
                },
                sys::mpv_render_param {
                    type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                    data: ptr::null_mut(),
                },
            ];
            if let Err(e) = mpv_err(unsafe { sys::mpv_render_context_create(&mut render, handle, params.as_mut_ptr()) }) {
                return destroy_on_err(handle, e);
            }
        }
        let path = match source_path(source) {
            Ok(p) => p,
            Err(e) => {
                unsafe {
                    if !render.is_null() {
                        sys::mpv_render_context_free(render);
                    }
                    sys::mpv_terminate_destroy(handle);
                }
                return Err(e);
            }
        };
        if let Err(e) = command(handle, &["loadfile", &path, "replace"]) {
            unsafe {
                if !render.is_null() {
                    sys::mpv_render_context_free(render);
                }
                sys::mpv_terminate_destroy(handle);
            }
            return Err(e);
        }
        let _ = set_prop_flag(handle, "pause", true);
        Ok(Self {
            handle,
            render,
            seeking: None,
            await_frame: None,
            rate: 1.0,
            muted: false,
            volume: 1.0,
            audio_track: None,
            video_track: None,
            subtitle: SubtitleSelect::Unset,
            kind,
            size,
        })
    }

    fn position(&self) -> Option<f64> {
        get_prop_f64(self.handle, "time-pos")
    }

    fn pump(&mut self, c: &mut Control) -> bool {
        loop {
            let ev = unsafe { sys::mpv_wait_event(self.handle, 0.0) };
            if ev.is_null() {
                break;
            }
            let ev = unsafe { &*ev };
            match ev.event_id {
                sys::mpv_event_id_MPV_EVENT_NONE => break,
                sys::mpv_event_id_MPV_EVENT_SHUTDOWN => {
                    c.fail("mpv shut down".into());
                    return false;
                }
                sys::mpv_event_id_MPV_EVENT_END_FILE => {
                    // reason in event data; treat as end of playback when not seeking.
                    if self.seeking.is_none() && c.playing {
                        let restart = c.observe(c.duration.unwrap_or(c.time), true);
                        if restart {
                            let _ = command(self.handle, &["seek", "0", "absolute"]);
                            let _ = set_prop_flag(self.handle, "pause", false);
                        } else {
                            let _ = set_prop_flag(self.handle, "pause", true);
                        }
                    }
                }
                _ => {}
            }
        }
        true
    }

    fn reconcile(&mut self, c: &mut Control, now: Instant) {
        if self.muted != c.muted {
            let _ = set_prop_flag(self.handle, "mute", c.muted);
            self.muted = c.muted;
        }
        let vol = c.volume.clamp(0.0, 1.0) as f64;
        if (self.volume - vol).abs() > 1e-4 {
            // mpv volume is 0…100.
            let _ = set_prop_f64(self.handle, "volume", vol * 100.0);
            self.volume = vol;
        }
        let mut track_changed = false;
        if let Some(i) = c.audio_track
            && self.audio_track != Some(i)
        {
            let _ = set_prop_string(self.handle, "aid", &i.to_string());
            self.audio_track = Some(i);
            track_changed = true;
        }
        if let Some(i) = c.video_track
            && self.video_track != Some(i)
        {
            let _ = set_prop_string(self.handle, "vid", &i.to_string());
            self.video_track = Some(i);
            track_changed = true;
        }
        if c.subtitle != SubtitleSelect::Unset && c.subtitle != self.subtitle {
            match c.subtitle {
                SubtitleSelect::Off => {
                    let _ = set_prop_string(self.handle, "sid", "no");
                }
                SubtitleSelect::Track(i) => {
                    let _ = set_prop_string(self.handle, "sid", &i.to_string());
                }
                SubtitleSelect::Unset => {}
            }
            self.subtitle = c.subtitle;
            track_changed = true;
        }
        if track_changed {
            self.await_frame = (self.kind == Kind::Video).then_some(now);
        }
        if let Some(t) = c.seek.take() {
            let _ = command(self.handle, &["seek", &format!("{t:.6}"), "absolute"]);
            self.seeking = Some((now, t));
            self.await_frame = (self.kind == Kind::Video).then_some(now);
            c.time = t;
        }
        let rate = c.rate as f64;
        if c.playing && (self.rate - rate).abs() > 1e-3 {
            let _ = set_prop_f64(self.handle, "speed", rate.max(1e-3));
            self.rate = rate;
        }
        let _ = set_prop_flag(self.handle, "pause", !c.playing);
        let _ = set_prop_flag(self.handle, "loop-file", c.looping);

        if let Some((since, _)) = self.seeking
            && now.duration_since(since) > SEEK_PATIENCE
        {
            self.seeking = None;
        }
        if let Some(pos) = self.position() {
            if self.seeking.is_some_and(|(_, t)| (pos - t).abs() < 0.05) {
                self.seeking = None;
            }
            if self.seeking.is_none() {
                let at_end = get_prop_flag(self.handle, "eof-reached").unwrap_or(false);
                let restart = c.observe(pos, at_end);
                if restart {
                    let _ = command(self.handle, &["seek", "0", "absolute"]);
                    let _ = set_prop_flag(self.handle, "pause", false);
                }
            }
        }
        self.sync_meta(c);
    }

    /// Leave Loading once mpv knows duration (and, for video, a size).
    fn sync_meta(&mut self, c: &mut Control) {
        if let Some((w, h)) = self.video_size() {
            self.size = Some((w, h));
            if c.size.is_none() {
                c.size = Some((w, h));
            }
        }
        let Some(d) = get_prop_f64(self.handle, "duration").filter(|d| d.is_finite() && *d > 0.0) else {
            return;
        };
        if c.duration.is_none() {
            c.duration = Some(d);
        }
        if c.meta.is_none() {
            let size = match self.kind {
                Kind::Video => self.size.or(c.size),
                Kind::Audio => None,
            };
            let frame = get_prop_f64(self.handle, "container-fps")
                .filter(|f| f.is_finite() && *f > 0.0)
                .map(|f| 1.0 / f)
                .unwrap_or(1.0 / 30.0);
            c.open(Ok(Meta { duration: d, size, frame }));
        } else if let Some(meta) = c.meta.as_mut() {
            meta.duration = d;
            if meta.size.is_none() {
                meta.size = self.size.or(c.size);
                if meta.size.is_some() {
                    c.size = meta.size;
                }
            }
        }
    }

    fn video_size(&self) -> Option<(u32, u32)> {
        if let Some(s) = self.size.filter(|(w, h)| *w > 0 && *h > 0) {
            return Some(s);
        }
        for (wp, hp) in [
            ("dwidth", "dheight"),
            ("width", "height"),
            ("video-params/w", "video-params/h"),
        ] {
            if let (Some(w), Some(h)) = (get_prop_i64(self.handle, wp), get_prop_i64(self.handle, hp))
                && w > 0
                && h > 0
            {
                return Some((w as u32, h as u32));
            }
        }
        None
    }

    fn list_tracks(&self, kind: MediaTrackKind) -> Vec<MediaTrack> {
        let raw = match get_prop_string(self.handle, "track-list") {
            Some(s) => s,
            None => return Vec::new(),
        };
        let Ok(list) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        let Some(arr) = list.as_array() else {
            return Vec::new();
        };
        let want = match kind {
            MediaTrackKind::Audio => "audio",
            MediaTrackKind::Video => "video",
            MediaTrackKind::Subtitle => "sub",
        };
        arr.iter()
            .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some(want))
            .filter_map(|t| {
                let id = t.get("id")?.as_i64()? as i32;
                // mpv ids are 1-based; Player track indices are 0-based like GStreamer.
                let index = id.saturating_sub(1);
                let language = t.get("lang").and_then(|v| v.as_str()).map(str::to_owned);
                let title = t.get("title").and_then(|v| v.as_str()).map(str::to_owned);
                Some(MediaTrack { index, language, title })
            })
            .collect()
    }

    fn current_track(&self, kind: MediaTrackKind) -> Option<i32> {
        let prop = match kind {
            MediaTrackKind::Audio => "aid",
            MediaTrackKind::Video => "vid",
            MediaTrackKind::Subtitle => "sid",
        };
        let s = get_prop_string(self.handle, prop)?;
        if s == "no" || s == "auto" {
            return if kind == MediaTrackKind::Subtitle { None } else { None };
        }
        s.parse::<i32>().ok().map(|id| id.saturating_sub(1))
    }

    fn pull_frame(&mut self) -> Result<Option<ByteFrame>, String> {
        if self.render.is_null() {
            return Ok(None);
        }
        if self.size.is_none() {
            self.size = self.video_size();
        }
        unsafe { sys::mpv_render_context_update(self.render) };
        let (w, h) = match self.size {
            Some(s) if s.0 > 0 && s.1 > 0 => s,
            _ => return Ok(None),
        };
        // mpv prefers 64-byte stride/pointer alignment for the SW path.
        let stride = ((w as usize) * 4).next_multiple_of(64);
        let mut pixels = vec![0u8; stride * (h as usize)];
        let mut size = [w as c_int, h as c_int];
        let mut stride_param = stride;
        // bgr0 is BGR + padding; the "0" byte is uninitialized (often 0). Our video
        // shader premultiplies by alpha, so leave it 0 and the frame is invisible.
        let format = CString::new("bgr0").unwrap();
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                data: format.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                data: &mut stride_param as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                data: pixels.as_mut_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        mpv_err(unsafe { sys::mpv_render_context_render(self.render, params.as_mut_ptr()) })?;
        // Pack to tightly packed BGRA with opaque alpha for NativeSurface.
        let row = (w as usize) * 4;
        let mut packed = vec![0u8; row * (h as usize)];
        for y in 0..(h as usize) {
            let src = &pixels[y * stride..y * stride + row];
            let dst = &mut packed[y * row..(y + 1) * row];
            dst.copy_from_slice(src);
            for px in dst.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        self.await_frame = None;
        Ok(Some(ByteFrame {
            width: w,
            height: h,
            format: Format::Bgra,
            color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
            planes: [packed, Vec::new()],
        }))
    }
}

#[derive(Default)]
pub(crate) struct Native {
    session: Option<Session>,
    size: Option<(u32, u32)>,
    frame: Option<f64>,
}

static AUDIO: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
static AUDIO_BUSY: AtomicBool = AtomicBool::new(false);
static TICKING: AtomicBool = AtomicBool::new(false);

pub(super) fn created(shared: &Arc<Shared>) {
    if shared.kind == Kind::Audio {
        AUDIO.lock().unwrap().push(Arc::downgrade(shared));
        if playback_enabled() {
            drive(shared);
        }
    }
}

pub(super) fn enable() {
    let clips: Vec<Arc<Shared>> = AUDIO.lock().unwrap().iter().filter_map(Weak::upgrade).collect();
    for clip in clips {
        drive(&clip);
    }
}


fn wait_frame(session: &mut Session) -> Result<Option<ByteFrame>, String> {
    for _ in 0..120 {
        let mut c = Control::default();
        if !session.pump(&mut c) {
            return Err(c.failed.clone().unwrap_or_else(|| "mpv session failed".into()));
        }
        session.sync_meta(&mut c);
        if let Some(frame) = session.pull_frame()? {
            return Ok(Some(frame));
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    Ok(None)
}

pub(super) fn probe(
    native: &mut Native,
    source: &MediaSource,
    headers: &[(String, String)],
    kind: Kind,
) -> Result<Meta, String> {
    if !headers.is_empty() {
        return Err("Linux Player does not support MediaRequest headers yet".into());
    }
    let mut session = Session::open(source, kind, None)?;
    let mut c = Control::default();
    c.muted = true;
    for _ in 0..64 {
        if !session.pump(&mut c) {
            return Err(c.failed.clone().unwrap_or_else(|| "mpv session failed".into()));
        }
        session.sync_meta(&mut c);
        if c.opened() {
            break;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    session.reconcile(&mut c, Instant::now());
    let meta = c.meta.ok_or_else(|| "media has no duration".to_string())?;
    if kind == Kind::Video && meta.size.is_none() {
        let _ = wait_frame(&mut session)?;
        let mut c2 = Control::default();
        session.sync_meta(&mut c2);
        if let Some(m) = c2.meta {
            native.size = m.size;
            native.frame = Some(m.frame);
            return Ok(m);
        }
    }
    native.size = meta.size;
    native.frame = Some(meta.frame);
    Ok(meta)
}

pub(super) fn decode(
    native: &mut Native,
    source: &MediaSource,
    headers: &[(String, String)],
    at: f64,
) -> Result<ByteFrame, String> {
    if !headers.is_empty() {
        return Err("Linux Player does not support MediaRequest headers yet".into());
    }
    let mut session = Session::open(source, Kind::Video, native.size)?;
    let mut c = Control::default();
    c.muted = true;
    c.seek = Some(at.max(0.0));
    session.reconcile(&mut c, Instant::now());
    let frame = wait_frame(&mut session)?.ok_or_else(|| "timed out decoding frame".to_string())?;
    native.size = Some((frame.width, frame.height));
    native.frame = Some(native.frame.unwrap_or(1.0 / 30.0));
    Ok(frame)
}

pub(super) fn decode_preview_at(
    source: &MediaSource,
    headers: &[(String, String)],
    at: f64,
    max: u32,
    cached: Option<((u32, u32), f64)>,
) -> Result<(ByteFrame, ((u32, u32), f64)), String> {
    if !headers.is_empty() {
        return Err("Linux Player does not support MediaRequest headers yet".into());
    }
    let mut native = Native { size: cached.map(|c| c.0), frame: cached.map(|c| c.1), ..Default::default() };
    let frame = decode(&mut native, source, headers, at)?;
    let key = (
        native.size.ok_or_else(|| "video track has no size".to_string())?,
        native.frame.unwrap_or(1.0 / 30.0),
    );
    Ok((fit_bgra(frame, max), key))
}


pub(super) fn preload(shared: &Arc<Shared>) {
    if shared.kind == Kind::Audio || shared.ctl.lock().unwrap().failed.is_some() {
        return;
    }
    let mut native = shared.native.lock().unwrap();
    if native.session.is_some() {
        return;
    }
    let mut c = shared.ctl.lock().unwrap();
    let _ = ensure_session(shared, &mut native, &mut c);
}

pub(super) fn apply(shared: &Arc<Shared>) {
    if !playback_enabled() || shared.ctl.lock().unwrap().failed.is_some() {
        return;
    }
    if shared.kind == Kind::Audio {
        drive(shared);
        return;
    }
    let Ok(mut native) = shared.native.try_lock() else { return };
    {
        let mut c = shared.ctl.lock().unwrap();
        let want = native.session.is_none()
            && (c.audio_track.is_some()
                || c.video_track.is_some()
                || matches!(c.subtitle, SubtitleSelect::Off | SubtitleSelect::Track(_)));
        if want {
            let _ = ensure_session(shared, &mut native, &mut c);
        }
    }
    let Some(session) = native.session.as_mut() else { return };
    let mut c = shared.ctl.lock().unwrap();
    if !session.pump(&mut c) {
        native.session = None;
        return;
    }
    session.reconcile(&mut c, Instant::now());
}

pub(super) fn list_tracks(shared: &Arc<Shared>, kind: MediaTrackKind) -> Vec<MediaTrack> {
    if !playback_enabled() || shared.ctl.lock().unwrap().failed.is_some() {
        return Vec::new();
    }
    let mut native = shared.native.lock().unwrap();
    if native.session.is_none() {
        let mut c = shared.ctl.lock().unwrap();
        let _ = ensure_session(shared, &mut native, &mut c);
    }
    native.session.as_ref().map(|s| s.list_tracks(kind)).unwrap_or_default()
}

pub(super) fn current_track(shared: &Arc<Shared>, kind: MediaTrackKind) -> Option<i32> {
    let native = shared.native.lock().unwrap();
    native.session.as_ref().and_then(|s| s.current_track(kind))
}

pub(super) fn live_time(shared: &Shared) -> Option<f64> {
    if shared.kind != Kind::Audio {
        return None;
    }
    let native = shared.native.try_lock().ok()?;
    let session = native.session.as_ref()?;
    let c = shared.ctl.lock().unwrap();
    if !(c.playing && !c.ended) {
        return None;
    }
    let time = session.position()?;
    Some(c.duration.map_or(time, |d| time.clamp(0.0, d)))
}

fn ensure_session(shared: &Arc<Shared>, native: &mut Native, c: &mut Control) -> bool {
    if native.session.is_some() {
        return true;
    }
    if !shared.headers.is_empty() {
        c.fail("Linux Player does not support MediaRequest headers yet".into());
        return false;
    }
    match Session::open(&shared.source, shared.kind, native.size.or(c.size)) {
        Ok(mut session) => {
            let _ = set_prop_flag(session.handle, "mute", c.muted);
            session.muted = c.muted;
            let vol = c.volume.clamp(0.0, 1.0) as f64;
            let _ = set_prop_f64(session.handle, "volume", vol * 100.0);
            session.volume = vol;
            if c.time > 0.0 {
                let _ = command(session.handle, &["seek", &format!("{:.6}", c.time), "absolute"]);
            }
            // Drain early events so duration/size can show up before the first draw.
            for _ in 0..32 {
                if !session.pump(c) {
                    return false;
                }
                session.sync_meta(c);
                if c.opened() {
                    break;
                }
            }
            session.reconcile(c, Instant::now());
            if let Some(size) = session.size {
                native.size = Some(size);
            }
            native.session = Some(session);
            true
        }
        Err(e) => {
            c.fail(e);
            false
        }
    }
}

pub(super) fn pull(shared: &Arc<Shared>, surface: &NativeSurface, _target: Instant) -> bool {
    let now = Instant::now();
    let mut native = shared.native.lock().unwrap();
    {
        let mut c = shared.ctl.lock().unwrap();
        if !ensure_session(shared, &mut native, &mut c) {
            return false;
        }
    }
    {
        let Some(session) = native.session.as_mut() else { return false };
        let mut c = shared.ctl.lock().unwrap();
        if !session.pump(&mut c) {
            drop(c);
            native.session = None;
            return false;
        }
        session.reconcile(&mut c, now);
    }
    let frame = {
        let Some(session) = native.session.as_mut() else { return false };
        if session.kind == Kind::Video { session.pull_frame() } else { Ok(None) }
    };
    match frame {
        Ok(Some(frame)) => {
            let mut c = shared.ctl.lock().unwrap();
            let id = c.next_frame_id();
            if let Err(e) = surface.push_bytes(frame, id) {
                c.fail(e);
            }
        }
        Ok(None) => {}
        Err(e) => shared.ctl.lock().unwrap().fail(e),
    }
    let playing = shared.ctl.lock().unwrap().playing;
    let waiting = native.session.as_ref().is_some_and(|s| s.seeking.is_some() || s.await_frame.is_some());
    playing || waiting
}

pub(super) fn drive(shared: &Arc<Shared>) {
    if shared.kind != Kind::Audio || !playback_enabled() {
        return;
    }
    let now = Instant::now();
    {
        let mut native = shared.native.lock().unwrap();
        let mut c = shared.ctl.lock().unwrap();
        if c.failed.is_some() {
            return;
        }
        if !ensure_session(shared, &mut native, &mut c) {
            return;
        }
        let Some(session) = native.session.as_mut() else { return };
        if !session.pump(&mut c) {
            native.session = None;
            return;
        }
        session.reconcile(&mut c, now);
        let busy = c.playing || session.seeking.is_some() || !c.opened();
        AUDIO_BUSY.store(busy || AUDIO_BUSY.load(Ordering::SeqCst), Ordering::SeqCst);
    }
    shared.settle(true);
    if AUDIO_BUSY.load(Ordering::SeqCst) && !TICKING.swap(true, Ordering::SeqCst) {
        std::thread::Builder::new()
            .name("rune-audio-tick".into())
            .spawn(tick_audio)
            .expect("spawn audio tick");
    }
}

fn tick_audio() {
    loop {
        std::thread::sleep(AUDIO_TICK);
        if !AUDIO_BUSY.load(Ordering::SeqCst) {
            TICKING.store(false, Ordering::SeqCst);
            if !AUDIO_BUSY.load(Ordering::SeqCst) || TICKING.swap(true, Ordering::SeqCst) {
                return;
            }
        }
        let clips: Vec<Arc<Shared>> = {
            let audio = AUDIO.lock().unwrap();
            audio.iter().filter_map(Weak::upgrade).collect()
        };
        let mut busy = false;
        for clip in &clips {
            let now = Instant::now();
            let mut native = clip.native.lock().unwrap();
            let mut c = clip.ctl.lock().unwrap();
            if c.failed.is_some() {
                continue;
            }
            if ensure_session(clip, &mut native, &mut c)
                && let Some(session) = native.session.as_mut()
            {
                if !session.pump(&mut c) {
                    native.session = None;
                } else {
                    session.reconcile(&mut c, now);
                    busy |= c.playing || session.seeking.is_some() || !c.opened();
                }
            }
            drop(c);
            drop(native);
            clip.settle(true);
        }
        AUDIO_BUSY.store(busy, Ordering::SeqCst);
    }
}
