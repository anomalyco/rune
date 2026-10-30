//! libVLC for windowed playback; headless stills and scrub previews use the same player.
//!
//! Live clips use `libvlc_video_set_callbacks` (RV32 → BGRA into [`NativeSurface`]) and
//! libVLC's audio output. One media player, one clock.
use super::*;
use super::linux_still::fit_bgra;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const AUDIO_TICK: Duration = Duration::from_millis(50);
const SEEK_PATIENCE: Duration = Duration::from_secs(2);

#[allow(non_camel_case_types)]
mod ffi {
    use super::*;

    #[repr(C)]
    pub(super) struct libvlc_instance_t {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub(super) struct libvlc_media_t {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub(super) struct libvlc_media_player_t {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub(super) struct libvlc_event_manager_t {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub(super) struct libvlc_event_t {
        pub type_: c_int,
        _p_obj: *mut c_void,
        _u: [u64; 8],
    }
    #[repr(C)]
    pub(super) struct libvlc_track_description_t {
        pub i_id: c_int,
        pub psz_name: *mut c_char,
        pub p_next: *mut libvlc_track_description_t,
    }

    pub(super) type libvlc_time_t = i64;
    pub(super) type libvlc_callback_t = Option<unsafe extern "C" fn(*const libvlc_event_t, *mut c_void)>;
    pub(super) type libvlc_video_lock_cb =
        Option<unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> *mut c_void>;
    pub(super) type libvlc_video_unlock_cb =
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const *mut c_void)>;
    pub(super) type libvlc_video_display_cb = Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>;
    pub(super) type libvlc_video_format_cb = Option<
        unsafe extern "C" fn(*mut *mut c_void, *mut c_char, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> c_uint,
    >;
    pub(super) type libvlc_video_cleanup_cb = Option<unsafe extern "C" fn(*mut c_void)>;

    #[link(name = "vlc")]
    unsafe extern "C" {
        pub(super) fn libvlc_new(argc: c_int, argv: *const *const c_char) -> *mut libvlc_instance_t;
        pub(super) fn libvlc_errmsg() -> *const c_char;
        pub(super) fn libvlc_media_new_path(inst: *mut libvlc_instance_t, path: *const c_char) -> *mut libvlc_media_t;
        pub(super) fn libvlc_media_new_location(inst: *mut libvlc_instance_t, psz: *const c_char) -> *mut libvlc_media_t;
        pub(super) fn libvlc_media_release(media: *mut libvlc_media_t);
        pub(super) fn libvlc_media_parse(media: *mut libvlc_media_t);
        pub(super) fn libvlc_media_get_duration(media: *mut libvlc_media_t) -> libvlc_time_t;
        pub(super) fn libvlc_media_player_new_from_media(media: *mut libvlc_media_t) -> *mut libvlc_media_player_t;
        pub(super) fn libvlc_media_player_release(mp: *mut libvlc_media_player_t);
        pub(super) fn libvlc_media_player_play(mp: *mut libvlc_media_player_t) -> c_int;
        pub(super) fn libvlc_media_player_stop(mp: *mut libvlc_media_player_t);
        pub(super) fn libvlc_media_player_set_pause(mp: *mut libvlc_media_player_t, do_pause: c_int);
        pub(super) fn libvlc_media_player_get_time(mp: *mut libvlc_media_player_t) -> libvlc_time_t;
        pub(super) fn libvlc_media_player_set_time(mp: *mut libvlc_media_player_t, i_time: libvlc_time_t);
        pub(super) fn libvlc_media_player_get_length(mp: *mut libvlc_media_player_t) -> libvlc_time_t;
        pub(super) fn libvlc_media_player_set_rate(mp: *mut libvlc_media_player_t, rate: f32) -> c_int;
        pub(super) fn libvlc_media_player_event_manager(mp: *mut libvlc_media_player_t) -> *mut libvlc_event_manager_t;
        pub(super) fn libvlc_event_attach(
            em: *mut libvlc_event_manager_t,
            typ: c_int,
            cb: libvlc_callback_t,
            data: *mut c_void,
        ) -> c_int;
        pub(super) fn libvlc_event_detach(
            em: *mut libvlc_event_manager_t,
            typ: c_int,
            cb: libvlc_callback_t,
            data: *mut c_void,
        );
        pub(super) fn libvlc_video_set_callbacks(
            mp: *mut libvlc_media_player_t,
            lock: libvlc_video_lock_cb,
            unlock: libvlc_video_unlock_cb,
            display: libvlc_video_display_cb,
            opaque: *mut c_void,
        );
        pub(super) fn libvlc_video_set_format_callbacks(
            mp: *mut libvlc_media_player_t,
            setup: libvlc_video_format_cb,
            cleanup: libvlc_video_cleanup_cb,
        );
        pub(super) fn libvlc_video_get_size(
            mp: *mut libvlc_media_player_t,
            num: c_uint,
            px: *mut c_uint,
            py: *mut c_uint,
        ) -> c_int;
        pub(super) fn libvlc_audio_set_mute(mp: *mut libvlc_media_player_t, status: c_int);
        pub(super) fn libvlc_audio_set_volume(mp: *mut libvlc_media_player_t, volume: c_int) -> c_int;
        pub(super) fn libvlc_audio_get_track_description(
            mp: *mut libvlc_media_player_t,
        ) -> *mut libvlc_track_description_t;
        pub(super) fn libvlc_video_get_track_description(
            mp: *mut libvlc_media_player_t,
        ) -> *mut libvlc_track_description_t;
        pub(super) fn libvlc_video_get_spu_description(
            mp: *mut libvlc_media_player_t,
        ) -> *mut libvlc_track_description_t;
        pub(super) fn libvlc_track_description_list_release(list: *mut libvlc_track_description_t);
        pub(super) fn libvlc_audio_get_track(mp: *mut libvlc_media_player_t) -> c_int;
        pub(super) fn libvlc_audio_set_track(mp: *mut libvlc_media_player_t, i_track: c_int) -> c_int;
        pub(super) fn libvlc_video_get_track(mp: *mut libvlc_media_player_t) -> c_int;
        pub(super) fn libvlc_video_set_track(mp: *mut libvlc_media_player_t, i_track: c_int) -> c_int;
        pub(super) fn libvlc_video_get_spu(mp: *mut libvlc_media_player_t) -> c_int;
        pub(super) fn libvlc_video_set_spu(mp: *mut libvlc_media_player_t, i_spu: c_int) -> c_int;
    }
}
use ffi::*;

/// libvlc_MediaPlayerEndReached (`libvlc_MediaPlayerMediaChanged` = 0x100 + 9).
const LIBVLC_MEDIA_PLAYER_END_REACHED: c_int = 0x109;

fn errmsg() -> String {
    let p = unsafe { libvlc_errmsg() };
    if p.is_null() {
        "libvlc error".into()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn instance() -> Result<*mut libvlc_instance_t, String> {
    struct Inst(*mut libvlc_instance_t);
    // Process-lifetime instance; never released.
    unsafe impl Send for Inst {}
    unsafe impl Sync for Inst {}
    static INST: OnceLock<Result<Inst, String>> = OnceLock::new();
    let inst = INST.get_or_init(|| {
        let args = [
            CString::new("--no-xlib").unwrap(),
            CString::new("--quiet").unwrap(),
            CString::new("--no-video-title-show").unwrap(),
        ];
        let ptrs: Vec<*const c_char> = args.iter().map(|c| c.as_ptr()).collect();
        let p = unsafe { libvlc_new(ptrs.len() as c_int, ptrs.as_ptr()) };
        if p.is_null() {
            Err(format!("libvlc_new: {}", errmsg()))
        } else {
            Ok(Inst(p))
        }
    });
    match inst {
        Ok(i) => Ok(i.0),
        Err(e) => Err(e.clone()),
    }
}

fn source_media(inst: *mut libvlc_instance_t, source: &MediaSource) -> Result<*mut libvlc_media_t, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            let c = CString::new(path.to_string_lossy().as_ref()).map_err(|e| e.to_string())?;
            let m = unsafe { libvlc_media_new_path(inst, c.as_ptr()) };
            if m.is_null() { Err(format!("libvlc_media_new_path: {}", errmsg())) } else { Ok(m) }
        }
        MediaSource::Url(url) => {
            let c = CString::new(url.as_str()).map_err(|e| e.to_string())?;
            let m = unsafe { libvlc_media_new_location(inst, c.as_ptr()) };
            if m.is_null() { Err(format!("libvlc_media_new_location: {}", errmsg())) } else { Ok(m) }
        }
    }
}

/// Soft-decoded RV32 frames from VLC's video callbacks.
struct VideoSink {
    width: u32,
    height: u32,
    pitch: usize,
    write: Vec<u8>,
    latest: Mutex<Option<ByteFrame>>,
}

impl VideoSink {
    fn new() -> Self {
        Self { width: 0, height: 0, pitch: 0, write: Vec::new(), latest: Mutex::new(None) }
    }

    fn resize(&mut self, w: u32, h: u32, pitch: usize) {
        self.width = w;
        self.height = h;
        self.pitch = pitch;
        self.write.resize(pitch * h as usize, 0);
    }

    fn publish(&self) {
        if self.width == 0 || self.height == 0 || self.write.is_empty() {
            return;
        }
        let row = self.width as usize * 4;
        let mut packed = vec![0u8; row * self.height as usize];
        for y in 0..self.height as usize {
            let src = &self.write[y * self.pitch..y * self.pitch + row];
            let dst = &mut packed[y * row..(y + 1) * row];
            dst.copy_from_slice(src);
            // RV32's A byte is often 0; our video shader premultiplies by alpha.
            for px in dst.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        *self.latest.lock().unwrap() = Some(ByteFrame {
            width: self.width,
            height: self.height,
            format: Format::Bgra,
            color: ColorInfo {
                matrix: crate::native_surface::Matrix::Bt709,
                transfer: crate::native_surface::Transfer::Srgb,
            },
            planes: [packed, Vec::new()],
        });
    }

    fn take(&self) -> Option<ByteFrame> {
        self.latest.lock().unwrap().take()
    }
}

unsafe extern "C" fn format_cb(
    opaque: *mut *mut c_void,
    chroma: *mut c_char,
    width: *mut c_uint,
    height: *mut c_uint,
    pitches: *mut c_uint,
    lines: *mut c_uint,
) -> c_uint {
    unsafe {
        if opaque.is_null()
            || chroma.is_null()
            || width.is_null()
            || height.is_null()
            || pitches.is_null()
            || lines.is_null()
        {
            return 0;
        }
        let sink = &mut *(*opaque as *mut VideoSink);
        let w = (*width).max(1);
        let h = (*height).max(1);
        // Prefer RV32 (32-bit RGB). Pitch multiple of 32 as libvlc recommends.
        let pitch = ((w as usize * 4 + 31) / 32) * 32;
        let bytes = b"RV32";
        for (i, b) in bytes.iter().enumerate() {
            *chroma.add(i) = *b as c_char;
        }
        *pitches = pitch as c_uint;
        *lines = h;
        *width = w;
        *height = h;
        sink.resize(w, h, pitch);
        1
    }
}

unsafe extern "C" fn lock_cb(opaque: *mut c_void, planes: *mut *mut c_void) -> *mut c_void {
    unsafe {
        let sink = &mut *(opaque as *mut VideoSink);
        if !planes.is_null() {
            *planes = sink.write.as_mut_ptr() as *mut c_void;
        }
        ptr::null_mut()
    }
}

unsafe extern "C" fn display_cb(opaque: *mut c_void, _picture: *mut c_void) {
    unsafe {
        let sink = &*(opaque as *mut VideoSink);
        sink.publish();
    }
}

unsafe extern "C" fn on_end(event: *const libvlc_event_t, data: *mut c_void) {
    unsafe {
        if event.is_null() || data.is_null() {
            return;
        }
        if (*event).type_ == LIBVLC_MEDIA_PLAYER_END_REACHED {
            (*(data as *const AtomicBool)).store(true, Ordering::SeqCst);
        }
    }
}

struct Session {
    player: *mut libvlc_media_player_t,
    media: *mut libvlc_media_t,
    sink: Option<Box<VideoSink>>,
    ended: Arc<AtomicBool>,
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

unsafe impl Send for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            if !self.player.is_null() {
                let em = libvlc_media_player_event_manager(self.player);
                if !em.is_null() {
                    libvlc_event_detach(
                        em,
                        LIBVLC_MEDIA_PLAYER_END_REACHED,
                        Some(on_end),
                        Arc::as_ptr(&self.ended) as *mut c_void,
                    );
                }
                libvlc_media_player_stop(self.player);
                libvlc_media_player_release(self.player);
                self.player = ptr::null_mut();
            }
            if !self.media.is_null() {
                libvlc_media_release(self.media);
                self.media = ptr::null_mut();
            }
        }
    }
}

impl Session {
    fn open(source: &MediaSource, kind: Kind, size: Option<(u32, u32)>) -> Result<Self, String> {
        let inst = instance()?;
        let media = source_media(inst, source)?;
        unsafe { libvlc_media_parse(media) };
        let player = unsafe { libvlc_media_player_new_from_media(media) };
        if player.is_null() {
            unsafe { libvlc_media_release(media) };
            return Err(format!("libvlc_media_player_new_from_media: {}", errmsg()));
        }

        let ended = Arc::new(AtomicBool::new(false));
        let em = unsafe { libvlc_media_player_event_manager(player) };
        if !em.is_null() {
            let err = unsafe {
                libvlc_event_attach(em, LIBVLC_MEDIA_PLAYER_END_REACHED, Some(on_end), Arc::as_ptr(&ended) as *mut c_void)
            };
            if err != 0 {
                unsafe {
                    libvlc_media_player_release(player);
                    libvlc_media_release(media);
                }
                return Err(format!("libvlc_event_attach: {}", errmsg()));
            }
        }

        let sink = if kind == Kind::Video {
            let mut sink = Box::new(VideoSink::new());
            if let Some((w, h)) = size.filter(|(w, h)| *w > 0 && *h > 0) {
                let pitch = ((w as usize * 4 + 31) / 32) * 32;
                sink.resize(w, h, pitch);
            }
            let opaque = sink.as_mut() as *mut VideoSink as *mut c_void;
            unsafe {
                libvlc_video_set_callbacks(player, Some(lock_cb), None, Some(display_cb), opaque);
                libvlc_video_set_format_callbacks(player, Some(format_cb), None);
            }
            Some(sink)
        } else {
            None
        };

        // Preroll: start then pause so length/size and the first frame are available.
        if unsafe { libvlc_media_player_play(player) } != 0 {
            drop(Session {
                player,
                media,
                sink,
                ended,
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
            });
            return Err(format!("libvlc_media_player_play: {}", errmsg()));
        }
        unsafe { libvlc_media_player_set_pause(player, 1) };

        Ok(Self {
            player,
            media,
            sink,
            ended,
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
        let ms = unsafe { libvlc_media_player_get_time(self.player) };
        (ms >= 0).then_some(ms as f64 / 1000.0)
    }

    fn duration_secs(&self) -> Option<f64> {
        let ms = unsafe { libvlc_media_player_get_length(self.player) };
        if ms > 0 {
            return Some(ms as f64 / 1000.0);
        }
        let ms = unsafe { libvlc_media_get_duration(self.media) };
        (ms > 0).then_some(ms as f64 / 1000.0)
    }

    fn video_size(&self) -> Option<(u32, u32)> {
        if let Some(s) = self.size.filter(|(w, h)| *w > 0 && *h > 0) {
            return Some(s);
        }
        if let Some(sink) = &self.sink
            && sink.width > 0
            && sink.height > 0
        {
            return Some((sink.width, sink.height));
        }
        let mut w = 0u32;
        let mut h = 0u32;
        if unsafe { libvlc_video_get_size(self.player, 0, &mut w, &mut h) } == 0 && w > 0 && h > 0 {
            return Some((w, h));
        }
        None
    }

    fn pump(&mut self, c: &mut Control) -> bool {
        if self.ended.swap(false, Ordering::SeqCst) && c.playing {
            let pos = self.position().unwrap_or(c.time);
            if c.observe(pos, true) {
                unsafe { libvlc_media_player_set_time(self.player, 0) };
                unsafe { libvlc_media_player_set_pause(self.player, 0) };
            }
        }
        true
    }

    fn reconcile(&mut self, c: &mut Control, now: Instant) {
        if self.muted != c.muted {
            unsafe { libvlc_audio_set_mute(self.player, c.muted as c_int) };
            self.muted = c.muted;
        }
        let vol = c.volume.clamp(0.0, 1.0) as f64;
        if (self.volume - vol).abs() > 1e-4 {
            let _ = unsafe { libvlc_audio_set_volume(self.player, (vol * 100.0).round() as c_int) };
            self.volume = vol;
        }
        let mut track_changed = false;
        if let Some(i) = c.audio_track
            && self.audio_track != Some(i)
        {
            let _ = unsafe { libvlc_audio_set_track(self.player, i) };
            self.audio_track = Some(i);
            track_changed = true;
        }
        if let Some(i) = c.video_track
            && self.video_track != Some(i)
        {
            let _ = unsafe { libvlc_video_set_track(self.player, i) };
            self.video_track = Some(i);
            track_changed = true;
        }
        if c.subtitle != SubtitleSelect::Unset && c.subtitle != self.subtitle {
            match c.subtitle {
                SubtitleSelect::Off => {
                    let _ = unsafe { libvlc_video_set_spu(self.player, -1) };
                }
                SubtitleSelect::Track(i) => {
                    let _ = unsafe { libvlc_video_set_spu(self.player, i) };
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
            unsafe { libvlc_media_player_set_time(self.player, (t.max(0.0) * 1000.0) as libvlc_time_t) };
            self.seeking = Some((now, t));
            self.await_frame = (self.kind == Kind::Video).then_some(now);
            c.time = t;
        }
        let rate = c.rate as f64;
        if c.playing && (self.rate - rate).abs() > 1e-3 {
            let _ = unsafe { libvlc_media_player_set_rate(self.player, rate.max(1e-3) as f32) };
            self.rate = rate;
        }
        unsafe { libvlc_media_player_set_pause(self.player, (!c.playing) as c_int) };

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
                let at_end = c.duration.is_some_and(|d| pos >= d - 0.05) || self.ended.load(Ordering::SeqCst);
                let restart = c.observe(pos, at_end);
                if restart {
                    self.ended.store(false, Ordering::SeqCst);
                    unsafe { libvlc_media_player_set_time(self.player, 0) };
                    unsafe { libvlc_media_player_set_pause(self.player, 0) };
                }
            }
        }
        self.sync_meta(c);
    }

    fn sync_meta(&mut self, c: &mut Control) {
        if let Some((w, h)) = self.video_size() {
            self.size = Some((w, h));
            if c.size.is_none() {
                c.size = Some((w, h));
            }
        }
        let Some(d) = self.duration_secs().filter(|d| d.is_finite() && *d > 0.0) else {
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
            c.open(Ok(Meta { duration: d, size, frame: 1.0 / 30.0 }));
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

    fn list_tracks(&self, kind: MediaTrackKind) -> Vec<MediaTrack> {
        let head = unsafe {
            match kind {
                MediaTrackKind::Audio => libvlc_audio_get_track_description(self.player),
                MediaTrackKind::Video => libvlc_video_get_track_description(self.player),
                MediaTrackKind::Subtitle => libvlc_video_get_spu_description(self.player),
            }
        };
        if head.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cur = head;
        while !cur.is_null() {
            let t = unsafe { &*cur };
            // Skip the "Disable" sentinel (id -1) for audio/video lists.
            if t.i_id >= 0 {
                let title = if t.psz_name.is_null() {
                    None
                } else {
                    Some(unsafe { CStr::from_ptr(t.psz_name) }.to_string_lossy().into_owned())
                };
                out.push(MediaTrack { index: t.i_id, language: None, title });
            }
            cur = t.p_next;
        }
        unsafe { libvlc_track_description_list_release(head) };
        out
    }

    fn current_track(&self, kind: MediaTrackKind) -> Option<i32> {
        let id = unsafe {
            match kind {
                MediaTrackKind::Audio => libvlc_audio_get_track(self.player),
                MediaTrackKind::Video => libvlc_video_get_track(self.player),
                MediaTrackKind::Subtitle => libvlc_video_get_spu(self.player),
            }
        };
        (id >= 0).then_some(id)
    }

    fn pull_frame(&mut self) -> Result<Option<ByteFrame>, String> {
        let Some(sink) = &self.sink else { return Ok(None) };
        let frame = sink.take();
        if frame.is_some() {
            self.await_frame = None;
            if self.size.is_none() {
                self.size = Some((sink.width, sink.height)).filter(|(w, h)| *w > 0 && *h > 0);
            }
        }
        Ok(frame)
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
            return Err(c.failed.clone().unwrap_or_else(|| "libvlc session failed".into()));
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
            return Err(c.failed.clone().unwrap_or_else(|| "libvlc session failed".into()));
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
            unsafe { libvlc_audio_set_mute(session.player, c.muted as c_int) };
            session.muted = c.muted;
            let vol = c.volume.clamp(0.0, 1.0) as f64;
            let _ = unsafe { libvlc_audio_set_volume(session.player, (vol * 100.0).round() as c_int) };
            session.volume = vol;
            if c.time > 0.0 {
                unsafe { libvlc_media_player_set_time(session.player, (c.time * 1000.0) as libvlc_time_t) };
            }
            for _ in 0..8 {
                if !session.pump(c) {
                    return false;
                }
                session.sync_meta(c);
                if c.opened() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(16));
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
