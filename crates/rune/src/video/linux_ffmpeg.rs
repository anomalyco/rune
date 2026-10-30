//! Shared ffmpeg/ffprobe helpers for Linux Player backends (GStreamer and libmpv).
//! Windowed playback uses the live backend; posters and headless frames still decode here.
use super::*;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub(super) const POSTER_MAX: u32 = 1920;

pub(super) fn tool_missing(bin: &str, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!("{bin} isn't on PATH (install ffmpeg; Arch: pacman -S ffmpeg; Debian/Ubuntu: apt install ffmpeg)")
    } else {
        format!("{bin}: {e}")
    }
}

pub(super) fn open_path(source: &MediaSource) -> Result<PathBuf, String> {
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

pub(super) fn open_meta(source: &MediaSource, kind: Kind) -> Result<Meta, String> {
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
pub(super) fn decode_at(source: &MediaSource, at: f64, size: (u32, u32), frame: f64, max: Option<u32>) -> Result<ByteFrame, String> {
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

#[cfg(test)]
#[test]
fn ffmpeg_decodes_the_rgbw_fixture() {
    use std::path::Path;
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
