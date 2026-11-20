use crate::native_surface::Format;
use crate::{PlayerState, Video};
#[cfg(target_os = "macos")]
use crate::Player;
#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rgbw.mp4")
}

#[cfg(target_os = "macos")]
fn sound(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn view(v: &Video, _: &mut crate::Cx<Video>) -> crate::El {
    crate::el().key("clip").w(32.0).h(18.0).video(v, crate::SurfaceFit::Fit)
}

/// The clip at the top of a list that scrolls it out of sight.
fn scrolled(v: &Video, _: &mut crate::Cx<Video>) -> crate::El {
    crate::col().key("list").w(32.0).h(18.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
        crate::col().w_full()
            .child(crate::el().key("clip").w(32.0).h(18.0).video(v, crate::SurfaceFit::Fit))
            .child(crate::el().w_full().h(200.0)),
    )
}

#[test]
fn failures_draw_a_placeholder_and_stop_drawing() {
    let v = Video::file("/nonexistent/clip.mp4");
    let mut rt = crate::Runtime::new(v.clone(), view, (32.0, 18.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    let PlayerState::Failed(message) = v.state() else { panic!("{:?}", v.state()) };
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    assert!(message.contains("no such file"), "{message}");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    assert_eq!(message, "video playback isn't supported on this platform yet");
    let frame = v.surface().latest().expect("placeholder frame");
    assert_eq!((frame.size, frame.format), ([32, 18], Format::Bgra));
    rt.frame(0.1);
    assert!(!rt.frame(0.1), "a failed video requests no frames");
}

#[test]
fn a_failed_clip_scrolling_across_the_clip_edge_commits_nothing() {
    let v = Video::file("/nonexistent/clip.mp4");
    let mut rt = crate::Runtime::new(v.clone(), scrolled, (32.0, 18.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(0.1);
    assert!(matches!(v.state(), PlayerState::Failed(_)));
    let commits = rt.stats.commits;
    for dy in [-40.0, 40.0, -40.0, 40.0] {
        let shown = rt.surfaces.len();
        rt.wheel(10.0, 10.0, dy);
        rt.frame(0.1);
        assert_ne!(rt.surfaces.len(), shown, "crossed the edge ({dy})");
    }
    assert_eq!(rt.stats.commits, commits, "scrolling across the edge commits nothing");
}

#[cfg(target_os = "macos")]
mod headless {
    use super::*;
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

    fn render(gpu: &Gpu, rt: &mut crate::Runtime<Video>) -> Vec<u8> {
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 36));
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (64, 36));
        let target = Offscreen::new(&gpu.device, 64, 36);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
        target.read(&gpu.device)
    }

    fn centre(p: &[u8]) -> [u8; 3] {
        let i = (18 * 64 + 32) * 4;
        [p[i], p[i + 1], p[i + 2]]
    }

    fn near(got: [u8; 3], want: [u8; 3], what: &str) {
        assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 24), "{what}: got {got:?}, want ~{want:?}");
    }

    /// The fixture is four 0.25 s frames: red, green, blue, white.
    #[test]
    fn headless_playback_decodes_the_frame_at_virtual_time() {
        let gpu = Gpu::new(Gpu::instance(), None);
        let v = Video::file(fixture());
        let mut rt = crate::Runtime::new(v.clone(), view, (32.0, 18.0), crate::Env::new(true, 2.0));
        rt.frame(0.0);
        assert_eq!(v.state(), PlayerState::Ready);
        assert_eq!(v.natural_size(), Some((64, 36)));
        assert!((v.duration().unwrap() - 1.0).abs() < 1e-3);
        let first = render(&gpu, &mut rt);
        near(centre(&first), [255, 0, 0], "first frame");
        rt.frame(0.0);
        assert!(!rt.frame(0.5), "a paused video requests no frames");
        assert_eq!(render(&gpu, &mut rt), first, "deterministic");

        v.play();
        assert!(rt.frame(0.0), "playing keeps frames coming");
        assert!(rt.frame(0.3));
        assert_eq!(v.state(), PlayerState::Playing);
        assert!((v.time() - 0.3).abs() < 1e-9);
        near(centre(&render(&gpu, &mut rt)), [0, 255, 0], "0.3 s");
        rt.frame(0.3);
        near(centre(&render(&gpu, &mut rt)), [0, 0, 255], "0.6 s");
        let tree = crate::tooling::tree_json(&rt, 0.6);
        let clip = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "clip").unwrap();
        assert_eq!(clip["video"]["state"], "playing");
        assert_eq!(clip["video"]["time"], 0.6);
        assert_eq!(clip["video"]["size"], serde_json::json!([64, 36]));
        assert_eq!(clip["video"]["muted"], true);

        rt.frame(0.3);
        near(centre(&render(&gpu, &mut rt)), [255, 255, 255], "0.9 s");
        rt.frame(0.3);
        assert_eq!(v.state(), PlayerState::Ended);
        assert_eq!(v.time(), 1.0);
        near(centre(&render(&gpu, &mut rt)), [255, 255, 255], "the end holds the last frame");
        rt.frame(0.0);
        assert!(!rt.frame(0.1), "an ended video requests no frames");

        v.seek(0.3);
        rt.frame(0.0);
        near(centre(&render(&gpu, &mut rt)), [0, 255, 0], "seek while paused");
        v.set_looping(true);
        v.set_rate(2.0);
        v.play();
        rt.frame(0.0);
        rt.frame(0.5);
        assert!((v.time() - 0.3).abs() < 1e-9, "0.3 + 2 x 0.5 wraps to 0.3: {}", v.time());
        near(centre(&render(&gpu, &mut rt)), [0, 255, 0], "looped");
    }

    /// A playing clip scrolled out of its list and back commits nothing, and shows the frame
    /// at the time it comes back, not the one it left on.
    #[test]
    fn a_playing_clip_scrolling_across_the_clip_edge_commits_nothing() {
        let gpu = Gpu::new(Gpu::instance(), None);
        let v = Video::file(fixture());
        v.set_looping(true);
        let mut rt = crate::Runtime::new(v.clone(), scrolled, (32.0, 18.0), crate::Env::new(true, 2.0));
        v.play();
        rt.frame(0.0);
        rt.frame(0.1);
        near(centre(&render(&gpu, &mut rt)), [255, 0, 0], "0.1 s");
        let commits = rt.stats.commits;
        rt.wheel(10.0, 10.0, -40.0);
        rt.frame(0.1);
        assert!(rt.surfaces.is_empty(), "scrolled out of sight");
        rt.frame(0.4);
        rt.wheel(10.0, 10.0, 40.0);
        rt.frame(0.0);
        assert_eq!(rt.surfaces.len(), 1, "back in sight");
        near(centre(&render(&gpu, &mut rt)), [0, 0, 255], "0.6 s, pulled as it comes back");
        assert_eq!(v.state(), PlayerState::Playing);
        assert_eq!(rt.stats.commits, commits, "scrolling across the edge commits nothing");
    }
}

/// A local HTTP server that serves `body` (with byte ranges) only to requests carrying
/// `Authorization: Bearer let-me-in`, and counts the ones that didn't.
#[cfg(all(target_os = "macos", feature = "ogg-audio"))]
fn serve_with_auth(body: Vec<u8>, content_type: &'static str) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/media/clip", listener.local_addr().unwrap().port());
    let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = refused.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (body, count) = (body.clone(), count.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let (mut authorized, mut range) = (false, None);
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 { return; }
                        let header = header.trim_end();
                        if header.is_empty() { break; }
                        let (name, value) = header.split_once(':').unwrap_or((header, ""));
                        match name.trim().to_ascii_lowercase().as_str() {
                            "authorization" => authorized = value.trim() == "Bearer let-me-in",
                            "range" => range = value.trim().strip_prefix("bytes=").map(str::to_owned),
                            _ => {}
                        }
                    }
                    if !authorized {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").ok();
                        continue;
                    }
                    let len = body.len();
                    let (start, end) = range.and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        let a: usize = a.parse().ok()?;
                        let b = b.parse::<usize>().map_or(len - 1, |b| b.min(len - 1));
                        Some((a, b))
                    }).unwrap_or((0, len - 1));
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\n\r\n",
                        end + 1 - start
                    );
                    if stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body[start..=end])).is_err() { return; }
                }
            });
        }
    });
    (url, refused)
}

/// Ogg and WebM audio, which AVFoundation can't read, decode and open like any clip: by file
/// extension, by URL with request headers, and by their bytes when the URL has no extension.
/// Each fixture is 0.5 s.
#[cfg(all(target_os = "macos", feature = "ogg-audio"))]
#[test]
fn headless_ogg_and_webm_audio_open_and_follow_the_clock() {
    let near = |a: &Player, what: &str| assert!((a.duration().unwrap_or(0.0) - 0.5).abs() < 0.03, "{what}: {:?} {:?}", a.state(), a.duration());
    for name in ["sine-vorbis.ogg", "sine-opus.ogg", "sine-opus.webm"] {
        let a = Player::audio(sound(name));
        assert_eq!(a.state(), PlayerState::Ready, "{name}");
        near(&a, name);
    }
    let (url, refused) = serve_with_auth(std::fs::read(sound("sine-opus.webm")).unwrap(), "application/octet-stream");
    let by_url = Player::request(format!("{url}.webm")).header("Authorization", "Bearer let-me-in").audio();
    near(&by_url, "url");
    let sniffed = Player::request(url).header("Authorization", "Bearer let-me-in").audio();
    near(&sniffed, "no extension");
    assert_eq!(refused.load(std::sync::atomic::Ordering::SeqCst), 0, "every request carried the header");

    let a = Player::audio(sound("sine-vorbis.ogg"));
    let mut rt = crate::Runtime::new((), |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0), (10.0, 10.0), crate::Env::new(true, 2.0));
    a.play();
    rt.frame(0.0);
    rt.frame(0.2);
    assert!((a.time() - 0.2).abs() < 1e-6, "{}", a.time());
    rt.frame(0.5);
    assert_eq!(a.state(), PlayerState::Ended);

    let broken = Player::audio("/nonexistent/clip.ogg");
    assert!(matches!(broken.state(), PlayerState::Failed(e) if e.contains("can't read")), "{:?}", broken.state());
    // Ogg bytes that don't decode, at a URL with no extension, report the decode error.
    let mut bytes = b"OggS".to_vec();
    bytes.extend([0x55; 400]);
    let (url, _) = serve_with_auth(bytes, "application/octet-stream");
    let garbled = Player::request(url).header("Authorization", "Bearer let-me-in").audio();
    assert!(matches!(garbled.state(), PlayerState::Failed(e) if e.contains("can't decode")), "{:?}", garbled.state());
}

/// Headless audio follows the runtime clock silently: play, pause, rate, seek and the end.
#[cfg(target_os = "macos")]
#[test]
fn headless_audio_time_follows_the_virtual_clock() {
    let a = Player::audio(sound("silence.wav"));
    let ends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let heard = ends.clone();
    let clip = a.clone();
    a.set_waker(Arc::new(move || {
        if clip.surface().video().is_some_and(|v| v.snapshot().state == PlayerState::Ended) {
            heard.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    let mut rt = crate::Runtime::new((), |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0), (10.0, 10.0), crate::Env::new(true, 2.0));
    let near = |t: f64, want: f64, what: &str| assert!((t - want).abs() < 1e-6, "{what}: {t}, want {want}");
    a.play();
    rt.frame(0.0);
    rt.frame(0.1);
    near(a.time(), 0.1, "playing");
    assert_eq!(a.state(), PlayerState::Playing);
    a.pause();
    rt.frame(0.2);
    near(a.time(), 0.1, "paused");
    a.set_rate(2.0);
    a.play();
    rt.frame(0.05);
    near(a.time(), 0.2, "double speed");
    a.seek(0.05);
    near(a.time(), 0.05, "seek");
    rt.frame(0.1);
    near(a.time(), 0.25, "plays on from the seek");
    assert_eq!(ends.load(std::sync::atomic::Ordering::SeqCst), 0);
    rt.frame(0.5);
    assert_eq!(a.state(), PlayerState::Ended);
    near(a.time(), 0.5, "holds the end");
    assert_eq!(ends.load(std::sync::atomic::Ordering::SeqCst), 1, "the waker hears the end once");
    a.set_looping(true);
    a.set_rate(1.0);
    a.play();
    rt.frame(0.6);
    near(a.time(), 0.1, "loops");
    assert!(a.is_playing());
}

/// A new runtime's clock starts at 0, never where an earlier runtime on this thread left off.
#[cfg(target_os = "macos")]
#[test]
fn a_new_runtime_starts_the_headless_clock_again() {
    let view = |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0);
    let mut old = crate::Runtime::new((), view, (10.0, 10.0), crate::Env::new(true, 2.0));
    old.frame(5.0);
    drop(old);
    let a = Player::audio(sound("silence.wav"));
    let mut rt = crate::Runtime::new((), view, (10.0, 10.0), crate::Env::new(true, 2.0));
    a.play();
    rt.frame(0.1);
    assert!((a.time() - 0.1).abs() < 1e-6, "{}", a.time());
}
