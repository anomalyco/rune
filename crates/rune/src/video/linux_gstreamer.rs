//! GStreamer for windowed playback; headless stills and scrub previews use the same playbin.
//!
//! Live clips use `playbin` with an `appsink` video sink (BGRA into [`NativeSurface`]) and the
//! default audio sink for sound — one pipeline, one clock.
use super::*;
use super::linux_still::fit_bgra;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::VideoFrameExt;
use std::sync::Once;
use std::time::Duration;

const AUDIO_TICK: Duration = Duration::from_millis(50);
const SEEK_PATIENCE: Duration = Duration::from_secs(2);
const STATE_WAIT: gst::ClockTime = gst::ClockTime::from_seconds(5);

static GST: Once = Once::new();

fn init_gst() -> Result<(), String> {
    static FAIL: Mutex<Option<String>> = Mutex::new(None);
    GST.call_once(|| {
        if let Err(e) = gst::init() {
            *FAIL.lock().unwrap() = Some(format!("gstreamer init: {e}"));
        }
    });
    match FAIL.lock().unwrap().as_ref() {
        Some(e) => Err(e.clone()),
        None => Ok(()),
    }
}

fn source_uri(source: &MediaSource) -> Result<String, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            glib::filename_to_uri(&path, None).map_err(|e| format!("uri: {e}")).map(|u| u.to_string())
        }
        MediaSource::Url(url) => Ok(url.clone()),
    }
}

/// One playbin session: video frames via appsink, audio via the default sink.
struct Session {
    playbin: gst::Element,
    appsink: Option<gst_app::AppSink>,
    seeking: Option<(Instant, f64)>,
    /// Keep pulling after a seek until appsink yields a frame (paused scrub otherwise stalls).
    await_frame: Option<Instant>,
    rate: f64,
    muted: bool,
    volume: f64,
    audio_track: Option<i32>,
    video_track: Option<i32>,
    subtitle: SubtitleSelect,
    kind: Kind,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

impl Session {
    fn open(source: &MediaSource, kind: Kind) -> Result<Self, String> {
        init_gst()?;
        let uri = source_uri(source)?;
        let playbin = gst::ElementFactory::make("playbin")
            .name("rune-playbin")
            .property("uri", uri)
            .build()
            .map_err(|e| format!("playbin: {e}"))?;

        // Keep pitch steady when seek rate ≠ 1 (otherwise voices chipmunk). Needs gst-plugins-good.
        if let Ok(scaletempo) = gst::ElementFactory::make("scaletempo").build() {
            playbin.set_property("audio-filter", &scaletempo);
        }

        // Typed GstPlayFlags (not gint). Keep text so subtitles composite into appsink frames;
        // soft-volume so Player::set_volume works without a hardware mixer.
        playbin.set_property_from_str(
            "flags",
            "video+audio+text+soft-volume+deinterlace+soft-colorbalance",
        );

        let appsink = if kind == Kind::Video {
            // sync=true so playback follows the clock (sync=false races at decode speed).
            // Paused scrub uses try_pull_preroll after ASYNC_DONE.
            let sink = gst_app::AppSink::builder()
                .caps(
                    &gst::Caps::builder("video/x-raw")
                        .field("format", gst_video::VideoFormat::Bgra.to_str())
                        .build(),
                )
                .drop(true)
                .max_buffers(3)
                .sync(true)
                .enable_last_sample(true)
                .build();
            playbin.set_property("video-sink", &sink);
            Some(sink)
        } else {
            let fake = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .map_err(|e| format!("fakesink: {e}"))?;
            playbin.set_property("video-sink", &fake);
            None
        };

        // Preroll so duration/size queries and the first frame are ready.
        playbin.set_state(gst::State::Paused).map_err(|e| format!("gstreamer: {e}"))?;
        let (_ret, state, _pending) = playbin.state(STATE_WAIT);
        if state != gst::State::Paused && state != gst::State::Playing {
            if let Some(err) = bus_error(&playbin) {
                let _ = playbin.set_state(gst::State::Null);
                return Err(err);
            }
            let _ = playbin.set_state(gst::State::Null);
            return Err("timed out opening media".into());
        }

        Ok(Self {
            playbin,
            appsink,
            seeking: None,
            await_frame: None,
            rate: 1.0,
            muted: false,
            volume: 1.0,
            audio_track: None,
            video_track: None,
            subtitle: SubtitleSelect::Unset,
            kind,
        })
    }

    fn list_tracks(&self, kind: MediaTrackKind) -> Vec<MediaTrack> {
        let (n_prop, tags_signal) = match kind {
            MediaTrackKind::Audio => ("n-audio", "get-audio-tags"),
            MediaTrackKind::Video => ("n-video", "get-video-tags"),
            MediaTrackKind::Subtitle => ("n-text", "get-text-tags"),
        };
        let n: i32 = self.playbin.property(n_prop);
        (0..n.max(0))
            .map(|index| {
                let tags: Option<gst::TagList> = self.playbin.emit_by_name(tags_signal, &[&index]);
                let language = tags.as_ref().and_then(|t| {
                    t.get::<gst::tags::LanguageCode>()
                        .map(|v| v.get().to_string())
                        .or_else(|| t.get::<gst::tags::LanguageName>().map(|v| v.get().to_string()))
                });
                let title = tags.as_ref().and_then(|t| t.get::<gst::tags::Title>().map(|v| v.get().to_string()));
                MediaTrack { index, language, title }
            })
            .collect()
    }

    fn current_track(&self, kind: MediaTrackKind) -> Option<i32> {
        let prop = match kind {
            MediaTrackKind::Audio => "current-audio",
            MediaTrackKind::Video => "current-video",
            MediaTrackKind::Subtitle => "current-text",
        };
        let i: i32 = self.playbin.property(prop);
        (i >= 0).then_some(i)
    }

    fn pump_bus(&mut self, c: &mut Control) -> bool {
        let Some(bus) = self.playbin.bus() else { return true };
        while let Some(msg) = bus.pop() {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    c.fail(format!("gstreamer: {}", err.error()));
                    return false;
                }
                MessageView::Eos(_) => {
                    let pos = self.position().unwrap_or(c.time);
                    if c.observe(pos, true) {
                        let _ = self.seek_to(0.0, c.rate as f64);
                    }
                }
                // Flushing seek finished and sinks have prerolled — safe to pull the new still.
                MessageView::AsyncDone(_) => {
                    self.seeking = None;
                }
                _ => {}
            }
        }
        c.failed.is_none()
    }

    fn position(&self) -> Option<f64> {
        self.playbin.query_position::<gst::ClockTime>().map(|t| t.seconds_f64())
    }

    fn duration_secs(&self) -> Option<f64> {
        self.playbin.query_duration::<gst::ClockTime>().map(|t| t.seconds_f64())
    }

    fn seek_to(&self, seconds: f64, rate: f64) -> Result<(), String> {
        let rate = if rate.is_finite() && rate > 0.0 { rate } else { 1.0 };
        let t = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1_000_000_000.0) as u64);
        // ACCURATE: KEY_UNIT snaps to the previous keyframe (pause/resume and scrub look like a
        // jump back a few frames, and paused scrub may not show a new frame at all).
        self.playbin
            .seek(
                rate,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                t,
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .map_err(|e| format!("seek: {e}"))
    }

    fn reconcile(&mut self, c: &mut Control, now: Instant) {
        if self.muted != c.muted {
            self.playbin.set_property("mute", c.muted);
            self.muted = c.muted;
        }
        let vol = c.volume.clamp(0.0, 1.0) as f64;
        if (self.volume - vol).abs() > 1e-4 {
            self.playbin.set_property("volume", vol);
            self.volume = vol;
        }
        let mut track_changed = false;
        if let Some(i) = c.audio_track
            && self.audio_track != Some(i)
        {
            self.playbin.set_property("current-audio", i);
            self.audio_track = Some(i);
            track_changed = true;
        }
        if let Some(i) = c.video_track
            && self.video_track != Some(i)
        {
            self.playbin.set_property("current-video", i);
            self.video_track = Some(i);
            track_changed = true;
        }
        if track_changed {
            let pos = self.position().unwrap_or(c.time);
            let rate = (c.rate as f64).max(1e-3);
            if self.seek_to(pos, rate).is_ok() {
                self.seeking = Some((now, pos));
                self.await_frame = self.appsink.is_some().then_some(now);
            }
        }
        if c.subtitle != SubtitleSelect::Unset && c.subtitle != self.subtitle {
            // playbin's current-text -1 means *auto*, not off — toggle the text flag for Off.
            let flags = match c.subtitle {
                SubtitleSelect::Off => "video+audio+soft-volume+deinterlace+soft-colorbalance",
                SubtitleSelect::Track(_) | SubtitleSelect::Unset => {
                    "video+audio+text+soft-volume+deinterlace+soft-colorbalance"
                }
            };
            self.playbin.set_property_from_str("flags", flags);
            if let SubtitleSelect::Track(i) = c.subtitle {
                self.playbin.set_property("current-text", i);
            }
            self.subtitle = c.subtitle;
            // Flush so playsink rebuilds the subtitle overlay into the appsink frames.
            let pos = self.position().unwrap_or(c.time);
            let rate = (c.rate as f64).max(1e-3);
            if self.seek_to(pos, rate).is_ok() {
                self.seeking = Some((now, pos));
                self.await_frame = self.appsink.is_some().then_some(now);
            }
        }
        if let Some(t) = c.seek.take() {
            // Use the app rate even while paused — do not use session.rate (was zeroed on pause
            // and forced a resume seek that jumped a few frames back).
            let rate = (c.rate as f64).max(1e-3);
            if self.seek_to(t, rate).is_ok() {
                self.seeking = Some((now, t));
                self.await_frame = self.appsink.is_some().then_some(now);
                c.time = t;
            }
        }
        // Rate changes only while already playing. Pausing must not zero `self.rate` — otherwise
        // the next play does a seek just to restore rate 1 and snaps back a few frames.
        if c.playing && (self.rate - c.rate as f64).abs() > 1e-3 {
            let pos = self.position().unwrap_or(c.time);
            if self.seek_to(pos, c.rate as f64).is_ok() {
                self.rate = c.rate as f64;
                self.seeking = Some((now, pos));
                self.await_frame = self.appsink.is_some().then_some(now);
            }
        } else if c.playing {
            self.rate = c.rate as f64;
        }

        let target = if c.playing { gst::State::Playing } else { gst::State::Paused };
        let _ = self.playbin.set_state(target);

        // Seeking clears on AsyncDone. Position can move before preroll is ready — don't use it.
        if self.seeking.is_some_and(|(since, _)| now - since > SEEK_PATIENCE) {
            self.seeking = None;
        }
        if self.await_frame.is_some_and(|since| now - since > SEEK_PATIENCE) {
            self.await_frame = None;
        }

        if self.seeking.is_none()
            && let Some(pos) = self.position()
        {
            c.time = c.duration.map_or(pos, |d| pos.clamp(0.0, d));
        }

        if c.duration.is_none()
            && let Some(d) = self.duration_secs().filter(|d| *d > 0.0)
        {
            c.duration = Some(d);
            if c.meta.is_none() {
                c.open(Ok(Meta {
                    duration: d,
                    size: c.size,
                    frame: 1.0 / 30.0,
                }));
            } else if let Some(meta) = c.meta.as_mut() {
                meta.duration = d;
            }
        }
    }

    fn pull_frame(&self, playing: bool) -> Result<Option<ByteFrame>, String> {
        let Some(appsink) = &self.appsink else { return Ok(None) };
        // Mid-seek preroll can still be the pre-flush frame — wait for AsyncDone.
        if self.seeking.is_some() {
            return Ok(None);
        }
        let sample = if playing {
            // Playing: queued samples. Also accept preroll right after pause/seek→play.
            appsink
                .try_pull_sample(gst::ClockTime::ZERO)
                .or_else(|| appsink.try_pull_preroll(gst::ClockTime::ZERO))
        } else if self.await_frame.is_some() {
            // Paused scrub: short wait for post-seek preroll (playbin text overlay stays in-frame).
            // Do not fall back to last-sample — it can still be the pre-seek buffer.
            appsink.try_pull_preroll(gst::ClockTime::from_mseconds(16))
        } else {
            appsink.try_pull_preroll(gst::ClockTime::ZERO)
        };
        let Some(sample) = sample else { return Ok(None) };
        Self::sample_to_frame(&sample)
    }

    fn sample_to_frame(sample: &gst::Sample) -> Result<Option<ByteFrame>, String> {
        let buffer = sample.buffer().ok_or_else(|| "gstreamer: sample has no buffer".to_string())?;
        let caps = sample.caps().ok_or_else(|| "gstreamer: sample has no caps".to_string())?;
        let info = gst_video::VideoInfo::from_caps(caps).map_err(|e| format!("video info: {e}"))?;
        let frame = gst_video::VideoFrame::from_buffer_readable(buffer.copy(), &info)
            .map_err(|_| "gstreamer: could not map video frame".to_string())?;
        let w = frame.width();
        let h = frame.height();
        let stride = frame.plane_stride()[0] as usize;
        let data = frame.plane_data(0).map_err(|_| "gstreamer: no plane 0".to_string())?;
        let row = w as usize * 4;
        let mut bytes = vec![0u8; row * h as usize];
        for y in 0..h as usize {
            let src = y * stride;
            let dst = y * row;
            bytes[dst..dst + row].copy_from_slice(&data[src..src + row]);
        }
        Ok(Some(ByteFrame {
            width: w,
            height: h,
            format: Format::Bgra,
            color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
            planes: [bytes, Vec::new()],
        }))
    }
}

fn bus_error(playbin: &gst::Element) -> Option<String> {
    let bus = playbin.bus()?;
    while let Some(msg) = bus.pop() {
        if let gst::MessageView::Error(err) = msg.view() {
            return Some(format!("gstreamer: {}", err.error()));
        }
    }
    None
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

fn wait_frame(session: &mut Session, c: &mut Control, playing: bool) -> Result<Option<ByteFrame>, String> {
    for _ in 0..120 {
        if !session.pump_bus(c) {
            return Err(c.failed.clone().unwrap_or_else(|| "gstreamer session failed".into()));
        }
        session.reconcile(c, Instant::now());
        if let Some(frame) = session.pull_frame(playing)? {
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
    let mut session = Session::open(source, kind)?;
    let mut c = Control::default();
    c.muted = true;
    session.reconcile(&mut c, Instant::now());
    let d = session
        .duration_secs()
        .filter(|d| *d > 0.0)
        .ok_or_else(|| "media has no duration".to_string())?;
    let (size, frame) = match kind {
        Kind::Video => {
            let got = wait_frame(&mut session, &mut c, false)?;
            let size = got.as_ref().map(|f| (f.width, f.height));
            (size, 1.0 / 30.0)
        }
        Kind::Audio => (None, 1.0 / 30.0),
    };
    native.size = size;
    native.frame = Some(frame);
    Ok(Meta { duration: d, size, frame })
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
    let mut session = Session::open(source, Kind::Video)?;
    let mut c = Control::default();
    c.muted = true;
    c.seek = Some(at.max(0.0));
    session.reconcile(&mut c, Instant::now());
    let frame = wait_frame(&mut session, &mut c, false)?
        .ok_or_else(|| "timed out decoding frame".to_string())?;
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
    if !session.pump_bus(&mut c) {
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
    match Session::open(&shared.source, shared.kind) {
        Ok(mut session) => {
            session.playbin.set_property("mute", c.muted);
            session.muted = c.muted;
            let vol = c.volume.clamp(0.0, 1.0) as f64;
            session.playbin.set_property("volume", vol);
            session.volume = vol;
            if c.time > 0.0 {
                let _ = session.seek_to(c.time, if c.playing { c.rate as f64 } else { 1.0 });
            }
            if let Some(d) = session.duration_secs().filter(|d| *d > 0.0) {
                if !c.opened() {
                    let size = native.size.or(c.size);
                    c.open(Ok(Meta { duration: d, size, frame: native.frame.unwrap_or(1.0 / 30.0) }));
                } else {
                    c.duration = Some(d);
                }
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
    let mut c = shared.ctl.lock().unwrap();

    if c.failed.is_some() {
        native.session = None;
        return false;
    }

    // Park a paused session for preroll (first still) and so missing files fail immediately.
    if !ensure_session(shared, &mut native, &mut c) {
        return false;
    }
    let session = native.session.as_mut().unwrap();
    if !session.pump_bus(&mut c) {
        native.session = None;
        return false;
    }
    session.reconcile(&mut c, now);

    if session.kind == Kind::Video {
        match session.pull_frame(c.playing) {
            Ok(Some(bytes)) => {
                session.await_frame = None;
                let id = c.next_frame_id();
                if let Err(e) = surface.push_bytes(bytes, id) {
                    c.fail(e);
                    native.session = None;
                    return false;
                }
            }
            Ok(None) => {}
            Err(e) => {
                c.fail(e);
                native.session = None;
                return false;
            }
        }
    }

    // Keep the GStreamer session while paused; keep drawing through seeks until a frame lands.
    c.playing || session.seeking.is_some() || session.await_frame.is_some()
}

pub(super) fn drive(shared: &Arc<Shared>) {
    if step_audio(shared) {
        AUDIO_BUSY.store(true, Ordering::SeqCst);
        start_ticking();
    }
    shared.settle(true);
}

fn step_audio(shared: &Arc<Shared>) -> bool {
    let now = Instant::now();
    let Ok(mut native) = shared.native.try_lock() else { return true };
    let mut c = shared.ctl.lock().unwrap();
    if c.failed.is_some() {
        native.session = None;
        return false;
    }
    if !ensure_session(shared, &mut native, &mut c) {
        return false;
    }
    let session = native.session.as_mut().unwrap();
    if !session.pump_bus(&mut c) {
        native.session = None;
        return false;
    }
    session.reconcile(&mut c, now);
    c.playing || session.seeking.is_some() || session.await_frame.is_some()
}

fn start_ticking() {
    if TICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("rune-audio-tick".into())
        .spawn(|| {
            loop {
                std::thread::sleep(AUDIO_TICK);
                if !AUDIO_BUSY.load(Ordering::SeqCst) {
                    TICKING.store(false, Ordering::SeqCst);
                    if !AUDIO_BUSY.load(Ordering::SeqCst) || TICKING.swap(true, Ordering::SeqCst) {
                        return;
                    }
                }
                tick();
            }
        })
        .expect("spawn audio tick");
}

fn tick() {
    let clips: Vec<Arc<Shared>> = {
        let mut audio = AUDIO.lock().unwrap();
        audio.retain(|w| w.strong_count() > 0);
        audio.iter().filter_map(Weak::upgrade).collect()
    };
    let mut busy = false;
    for clip in &clips {
        busy |= step_audio(clip);
        clip.settle(true);
    }
    AUDIO_BUSY.store(busy, Ordering::SeqCst);
}

