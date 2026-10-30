//! GStreamer for windowed playback, ffmpeg/ffprobe on PATH for probe and headless frames.
//!
//! Live clips use `playbin` with an `appsink` video sink (BGRA into [`NativeSurface`]) and the
//! default audio sink for sound — one pipeline, one clock. Headless `--snap` / posters still
//! decode a single frame through ffmpeg so virtual time stays deterministic without a bus.
use super::*;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::VideoFrameExt;
use std::io::Read;
use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Once;
use std::time::Duration;

const POSTER_MAX: u32 = 1920;
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

fn tool_missing(bin: &str, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!("{bin} isn't on PATH (install ffmpeg; Arch: pacman -S ffmpeg; Debian/Ubuntu: apt install ffmpeg)")
    } else {
        format!("{bin}: {e}")
    }
}

fn open_path(source: &MediaSource) -> Result<PathBuf, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            Ok(std::path::absolute(path).unwrap_or_else(|_| path.clone()))
        }
        MediaSource::Url(url) => Ok(PathBuf::from(url)),
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

fn ffprobe_json(name: &str) -> Result<serde_json::Value, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", name])
        .output()
        .map_err(|e| tool_missing("ffprobe", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.lines().last().unwrap_or("ffprobe failed").trim();
        return Err(if msg.is_empty() { "ffprobe failed".into() } else { msg.into() });
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe json: {e}"))
}

fn stream<'a>(probe: &'a serde_json::Value, kind: &str) -> Option<&'a serde_json::Value> {
    probe.get("streams")?.as_array()?.iter().find(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some(kind))
}

fn num(v: &serde_json::Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn parse_rate(s: &str) -> Option<f64> {
    let (a, b) = s.split_once('/')?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (b > 0.0 && a > 0.0).then_some(b / a)
}

fn open_meta(source: &MediaSource, kind: Kind) -> Result<Meta, String> {
    let name = open_path(source)?.to_string_lossy().into_owned();
    let probe = ffprobe_json(&name)?;
    match kind {
        Kind::Audio => {
            let s = stream(&probe, "audio").ok_or_else(|| {
                "no playable audio track (unreachable, or a format FFmpeg can't read)".to_string()
            })?;
            let duration = num(s, "duration")
                .or_else(|| probe.get("format").and_then(|f| num(f, "duration")))
                .unwrap_or(0.0);
            Ok(Meta { duration, size: None, frame: 1.0 / 30.0 })
        }
        Kind::Video => {
            let s = stream(&probe, "video").ok_or_else(|| "no playable video track".to_string())?;
            let w = num(s, "width").unwrap_or(0.0).round() as u32;
            let h = num(s, "height").unwrap_or(0.0).round() as u32;
            if w < 1 || h < 1 {
                return Err("video track has no size".into());
            }
            let duration = num(s, "duration")
                .or_else(|| probe.get("format").and_then(|f| num(f, "duration")))
                .unwrap_or(0.0);
            let frame = s
                .get("avg_frame_rate")
                .and_then(|v| v.as_str())
                .and_then(parse_rate)
                .or_else(|| s.get("r_frame_rate").and_then(|v| v.as_str()).and_then(parse_rate))
                .filter(|f| f.is_finite() && *f > 0.0)
                .unwrap_or(1.0 / 30.0);
            Ok(Meta { duration, size: Some((w, h)), frame })
        }
    }
}

fn scale_size(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max || long == 0 {
        return (w.max(1), h.max(1));
    }
    let s = max as f64 / long as f64;
    ((w as f64 * s).round().max(1.0) as u32, (h as f64 * s).round().max(1.0) as u32)
}

fn seek_time(at: f64, frame: f64) -> f64 {
    let at = at.max(0.0);
    if frame > 0.0 { (at / frame).floor() * frame } else { at }
}

/// Single-frame decode for posters and headless virtual time.
fn decode_at(source: &MediaSource, at: f64, size: (u32, u32), frame: f64, max: Option<u32>) -> Result<ByteFrame, String> {
    let name = open_path(source)?.to_string_lossy().into_owned();
    let (w, h) = match max {
        Some(max) => scale_size(size.0, size.1, max),
        None => (size.0.max(1), size.1.max(1)),
    };
    let at = format!("{:.6}", seek_time(at, frame));
    let mut child = Command::new("ffmpeg")
        .args([
            "-nostdin", "-hide_banner", "-loglevel", "error",
            "-ss", &at, "-i", &name,
            "-frames:v", "1", "-an",
            "-f", "rawvideo", "-pix_fmt", "bgra",
            "-s", &format!("{w}x{h}"),
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tool_missing("ffmpeg", e))?;
    let mut stdout = child.stdout.take().ok_or_else(|| "ffmpeg: no stdout".to_string())?;
    let expected = (w as usize) * (h as usize) * 4;
    let mut bytes = Vec::with_capacity(expected);
    stdout.read_to_end(&mut bytes).map_err(|e| format!("ffmpeg read: {e}"))?;
    let status = child.wait().map_err(|e| format!("ffmpeg: {e}"))?;
    if !status.success() {
        return Err("ffmpeg failed to decode a frame".into());
    }
    if bytes.len() != expected {
        return Err(format!("ffmpeg returned {} bytes, expected {expected} for {w}x{h} BGRA", bytes.len()));
    }
    Ok(ByteFrame {
        width: w,
        height: h,
        format: Format::Bgra,
        color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
        planes: [bytes, Vec::new()],
    })
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
            // Paused scrub uses try_pull_preroll after ASYNC_DONE — not ffmpeg posters (no subs).
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
    posters: Arc<Mutex<Posters>>,
    poster_started: bool,
    poster_due: bool,
    size: Option<(u32, u32)>,
    frame: Option<f64>,
}

#[derive(Default)]
struct Posters {
    want: Option<f64>,
    busy: bool,
    done: Option<Result<(Option<ByteFrame>, Meta), String>>,
}

static AUDIO: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
static AUDIO_BUSY: AtomicBool = AtomicBool::new(false);
static TICKING: AtomicBool = AtomicBool::new(false);

pub(super) fn created(shared: &Arc<Shared>) {
    if shared.kind == Kind::Audio {
        AUDIO.lock().unwrap().push(Arc::downgrade(shared));
        if playback_enabled() {
            load_audio(shared);
        }
    }
}

pub(super) fn enable() {
    let clips: Vec<Arc<Shared>> = AUDIO.lock().unwrap().iter().filter_map(Weak::upgrade).collect();
    for clip in clips {
        load_audio(&clip);
        drive(&clip);
    }
}

fn load_audio(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    if !native.poster_started && !shared.ctl.lock().unwrap().opened() {
        request_poster(shared, &mut native, 0.0);
    }
}

fn request_poster(shared: &Arc<Shared>, native: &mut Native, at: f64) {
    native.poster_started = true;
    let mut slot = native.posters.lock().unwrap();
    slot.want = Some(at);
    if slot.busy {
        return;
    }
    slot.busy = true;
    let posters = native.posters.clone();
    let clip = Arc::downgrade(shared);
    let (source, kind) = (shared.source.clone(), shared.kind);
    std::thread::Builder::new()
        .name("rune-media-poster".into())
        .spawn(move || {
            let mut opened: Option<Meta> = None;
            loop {
                let Some(at) = ({
                    let mut slot = posters.lock().unwrap();
                    let want = slot.want.take();
                    if want.is_none() {
                        slot.busy = false;
                    }
                    want
                }) else {
                    return;
                };
                if clip.strong_count() == 0 {
                    return;
                }
                let result = (|| {
                    if opened.is_none() {
                        opened = Some(open_meta(&source, kind)?);
                    }
                    let meta = *opened.as_ref().unwrap();
                    let frame = match (kind, meta.size) {
                        (Kind::Video, Some(size)) => {
                            Some(decode_at(&source, meta.frame_time(meta.frame_index(at)), size, meta.frame, Some(POSTER_MAX))?)
                        }
                        (Kind::Video, None) => return Err("video track has no size".into()),
                        (Kind::Audio, _) => None,
                    };
                    Ok((frame, meta))
                })();
                if let Ok((_, meta)) = &result
                    && let Some(clip) = clip.upgrade()
                {
                    let mut native = clip.native.lock().unwrap();
                    native.size = meta.size;
                    native.frame = Some(meta.frame);
                }
                posters.lock().unwrap().done = Some(result);
                let Some(clip) = clip.upgrade() else { return };
                match kind {
                    Kind::Video => clip.wake_windows(),
                    Kind::Audio => drive(&clip),
                }
            }
        })
        .expect("spawn media poster worker");
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
    let meta = open_meta(source, kind)?;
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
    let (size, frame) = match (native.size, native.frame) {
        (Some(size), Some(frame)) => (size, frame),
        _ => {
            let meta = open_meta(source, Kind::Video)?;
            native.size = meta.size;
            native.frame = Some(meta.frame);
            (meta.size.ok_or_else(|| "video track has no size".to_string())?, meta.frame)
        }
    };
    decode_at(source, at, size, frame, None)
}

/// Scrub preview: never takes `Native`, so a worker cannot stall live `pull`.
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
    let (size, frame) = match cached {
        Some(c) => c,
        None => {
            let meta = open_meta(source, Kind::Video)?;
            (meta.size.ok_or_else(|| "video track has no size".to_string())?, meta.frame)
        }
    };
    Ok((decode_at(source, at, size, frame, Some(max))?, (size, frame)))
}

/// Decode the first frame now so the first draw already has a poster.
pub(super) fn preload(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    let c = shared.ctl.lock().unwrap();
    if shared.kind == Kind::Audio || native.session.is_some() || native.poster_started || c.failed.is_some() {
        return;
    }
    let at = c.time;
    drop(c);
    native.poster_due = true;
    request_poster(shared, &mut native, at);
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

fn take_poster(native: &mut Native, c: &mut Control, surface: Option<&NativeSurface>) {
    let Some(done) = native.posters.lock().unwrap().done.take() else { return };
    match done {
        Ok((bytes, meta)) => {
            if !c.opened() {
                c.open(Ok(meta));
            }
            native.size = meta.size.or(native.size);
            native.frame = Some(meta.frame);
            // Only when there is no live playbin session — posters omit the subtitle overlay.
            if let (Some(bytes), Some(surface), None, true) = (bytes, surface, &native.session, native.poster_due) {
                native.poster_due = false;
                let id = c.next_frame_id();
                if let Err(e) = surface.push_bytes(bytes, id) {
                    c.fail(e);
                }
            }
        }
        Err(e) => c.fail(e),
    }
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
            native.poster_started = true;
            native.poster_due = false;
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

    take_poster(&mut native, &mut c, Some(surface));
    if c.failed.is_some() {
        native.session = None;
        return false;
    }

    if native.session.is_none() && !c.playing {
        let want = c.seek.take().or_else(|| (!native.poster_started).then_some(c.time));
        if let Some(at) = want {
            drop(c);
            native.poster_due = true;
            request_poster(shared, &mut native, at);
        }
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
    take_poster(&mut native, &mut c, None);
    if c.failed.is_some() {
        native.session = None;
        return false;
    }
    if native.session.is_none() && !c.playing {
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

#[cfg(test)]
#[test]
fn ffmpeg_decodes_the_rgbw_fixture() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rgbw.mp4");
    let source = MediaSource::File(path);
    let meta = open_meta(&source, Kind::Video).unwrap();
    assert_eq!(meta.size, Some((64, 36)));
    assert!((meta.duration - 1.0).abs() < 1e-3);
    let size = meta.size.unwrap();
    let centre = |at: f64| -> [u8; 3] {
        let f = decode_at(&source, at, size, meta.frame, None).unwrap();
        let i = ((18 * 64 + 32) * 4) as usize;
        [f.planes[0][i + 2], f.planes[0][i + 1], f.planes[0][i]]
    };
    let near = |got: [u8; 3], want: [u8; 3], what: &str| {
        assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 24), "{what}: got {got:?}, want ~{want:?}");
    };
    near(centre(0.125), [255, 0, 0], "red");
    near(centre(0.375), [0, 255, 0], "green");
    near(centre(0.625), [0, 0, 255], "blue");
    near(centre(0.875), [255, 255, 255], "white");
}
