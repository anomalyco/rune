//! Shared-element motion with `El::matched`: a 2×2 grid of picture tiles. 1–4 picks a tile, ↩
//! (or a click) zooms it to the whole stage and Esc zooms it back. Both places are plain flex
//! layout; the tile flies between them because both carry the same `matched` id.
//!
//! ```sh
//! cargo run -p matched
//! cargo run -p matched -- --script examples/matched/scripts/zoom.json --frames 240 --out frames
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;
use rune::tok::{self, type_};
use rune::{AppSpec, Color, Curve, Cx, Easing, El, ImageCache, Key, KeyEvent, Pixels, SurfaceFit, TextStyle, Transition, Tx, Waker, col, el, row, text};

struct State {
    picked: usize,
    zoomed: bool,
    timed: bool,
    pictures: [Pixels; 4],
    mips_mode: bool,
    cache: Option<ImageCache>,
    streamed: Pixels,
    mipmapped: Pixels,
    spare: Vec<u8>,
    frame: u32,
    reused: u32,
}

/// Sky, ground and sun colours of each tile's picture, as sRGB bytes.
const SCENES: [([u8; 3], [u8; 3], [u8; 3]); 4] = [
    ([255, 166, 120], [58, 40, 92], [255, 236, 170]),
    ([120, 200, 255], [30, 90, 70], [255, 255, 235]),
    ([196, 150, 255], [40, 30, 70], [255, 200, 240]),
    ([255, 214, 120], [120, 60, 30], [255, 250, 220]),
];

const NAMES: [&str; 4] = ["Dusk", "Lake", "Haze", "Dune"];

fn picture(i: usize) -> Pixels {
    let (w, h) = (320u32, 240u32);
    let (sky, ground, sun) = SCENES[i];
    let mix = |a: [u8; 3], b: [u8; 3], t: f32| a.map(|v| v as f32).iter().zip(b).map(|(a, b)| a + (b as f32 - a) * t).collect::<Vec<_>>();
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let horizon = 0.62 + 0.04 * (u * 6.0 + i as f32).sin();
            // Ground coverage of this pixel, anti-aliased over one pixel row.
            let land = ((v - horizon) * h as f32 + 0.5).clamp(0.0, 1.0);
            let air = mix(sky, ground, (v / horizon).min(1.0).powf(2.2) * 0.5);
            let d = ((u - 0.3 - 0.12 * i as f32).powi(2) + ((v - 0.42) * 0.75).powi(2)).sqrt();
            let glow = (1.0 - (d / 0.16)).clamp(0.0, 1.0).powf(0.6);
            let air: Vec<f32> = air.iter().zip(sun).map(|(a, b)| a + (b as f32 - a) * glow).collect();
            let soil = mix(ground, [10, 10, 16], (v - horizon).max(0.0) * 1.6);
            rgba.extend(air.iter().zip(soil).map(|(a, s)| (a + (s - a) * land).clamp(0.0, 255.0) as u8));
            rgba.push(255);
        }
    }
    let p = Pixels::new();
    p.set(w, h, rgba);
    p
}

const BOARD_SIDE: u32 = 256;

fn fill_checkerboard(buf: &mut Vec<u8>, phase: u32) {
    let len = (BOARD_SIDE * BOARD_SIDE * 4) as usize;
    buf.resize(len, 0);
    for y in 0..BOARD_SIDE {
        for x in 0..BOARD_SIDE {
            let v = if (x + y + phase) % 2 == 0 { 0 } else { 255 };
            let i = ((y * BOARD_SIDE + x) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
}

impl State {
    fn step_stream(&mut self, wait_mips: bool) {
        self.frame = self.frame.wrapping_add(1);
        let len = (BOARD_SIDE * BOARD_SIDE * 4) as usize;
        let mut buf = std::mem::take(&mut self.spare);
        let reused = buf.capacity() >= len;
        fill_checkerboard(&mut buf, self.frame);
        let mip_bytes: Arc<[u8]> = buf.as_slice().into();
        self.spare = self.streamed.replace(BOARD_SIDE, BOARD_SIDE, buf);
        if reused {
            self.reused = self.reused.wrapping_add(1);
        }
        if wait_mips {
            let (tx, rx) = std::sync::mpsc::channel();
            let cache = ImageCache::new(Arc::new(move || { tx.send(()).ok(); }));
            cache.rgba_into(&self.mipmapped, BOARD_SIDE, BOARD_SIDE, mip_bytes);
            let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
        } else if let Some(cache) = &self.cache {
            cache.rgba_into(&self.mipmapped, BOARD_SIDE, BOARD_SIDE, mip_bytes);
        }
    }

    fn enter_mips(&mut self, wait_mips: bool) {
        self.mips_mode = true;
        if self.streamed.size() == (0, 0) {
            // Prime three frames: frame 1 fills `streamed`, frame 2 hands frame 1's Vec back into
            // `spare`, and frame 3 reuses `spare` with zero reallocation.
            self.step_stream(false);
            self.step_stream(false);
            self.step_stream(wait_mips);
        }
    }
}

fn init() -> State {
    State {
        picked: 0,
        zoomed: false,
        timed: false,
        pictures: std::array::from_fn(picture),
        mips_mode: false,
        cache: None,
        streamed: Pixels::new(),
        mipmapped: Pixels::new(),
        spare: Vec::new(),
        frame: 0,
        reused: 0,
    }
}

fn setup(s: &mut State, waker: Waker) {
    s.cache = Some(ImageCache::new(waker));
}

fn tile(s: &State, i: usize) -> El {
    let id = format!("tile-{}", i + 1);
    let mut t = el().key(id.as_str()).matched(id.as_str()).grow(1.0).radius(20.0).clip_rounded(20.0)
        .fill(Color::BLACK)
        .on_press(move |s: &mut State| {
            if s.zoomed { s.zoomed = false } else { (s.picked, s.zoomed) = (i, true) }
        })
        .child(el().w_full().h_full().pixels_fit(&s.pictures[i], SurfaceFit::Fill))
        .child(text(format!("{}  {}", i + 1, NAMES[i])).font(TextStyle::new(22.0, 600.0)).color(Color::WHITE).absolute(18.0, 14.0));
    if s.timed {
        t = t
            .matched_motion(Curve::tween(0.24, Easing::EASE_IN_OUT))
            .press(Color::WHITE.alpha(0.10))
            .press_scale(0.97)
            .press_motion(
                Curve::tween(0.12, Easing::EASE_OUT),
                Curve::tween(0.16, Easing::EASE_OUT),
            );
    }
    if s.picked == i {
        let mut ring = el().absolute(0.0, 0.0).w_full().h_full().radius(20.0).border(3.0, Color::WHITE);
        if s.timed {
            ring = ring.transition(Transition::opacity()).transition_motion(
                Curve::tween(0.18, Easing::EASE_OUT),
                Curve::tween(0.16, Easing::EASE_OUT),
            );
        }
        t = t.child(ring);
    }
    t
}

fn mips_card(title: &str, subtitle: &str, pixels: &Pixels, theme: &tok::Theme) -> El {
    col().grow(1.0).padding(18.0).gap(14.0).radius(16.0).fill(theme.card).border(1.0, theme.card_border)
        .child(text(title).font(TextStyle::new(16.0, 600.0)).color(theme.text))
        .child(text(subtitle).font(type_::LABEL).color(theme.text_secondary))
        .child(
            row().gap(16.0).items_end()
                .child(col().gap(6.0)
                    .child(el().size(37.0, 37.0).radius(4.0).clip_rounded(4.0).pixels(pixels))
                    .child(text("37×37 pt").font(type_::LABEL).color(theme.text_secondary)))
                .child(col().gap(6.0)
                    .child(el().size(64.0, 64.0).radius(6.0).clip_rounded(6.0).pixels(pixels))
                    .child(text("64×64 pt").font(type_::LABEL).color(theme.text_secondary)))
                .child(col().gap(6.0)
                    .child(el().size(112.0, 112.0).radius(8.0).clip_rounded(8.0).pixels(pixels))
                    .child(text("112×112 pt").font(type_::LABEL).color(theme.text_secondary))),
        )
}

fn mips_view(s: &State, theme: &tok::Theme) -> El {
    let sample = s.streamed.with_rgba(|w, h, bytes| {
        format!("{w}×{h} px · pixel[0]=({}, {}, {}, {})", bytes[0], bytes[1], bytes[2], bytes[3])
    }).unwrap_or_else(|| "empty".into());
    let stream_info = format!("frame {} · recycled {}× · with_rgba: {sample}", s.frame, s.reused);
    let mip_info = format!("{}×{} px · with_rgba: {:?}", s.mipmapped.size().0, s.mipmapped.size().1, s.mipmapped.with_rgba(|_, _, _| ()));
    let next_btn = el().key("next-frame").px(12.0).py(6.0).radius(8.0).fill(theme.card).border(1.0, theme.card_border)
        .role("button").label("Next frame")
        .on_press(|s: &mut State| s.step_stream(false))
        .child(text("Next frame (R)").font(type_::LABEL).color(theme.text));
    col().key("mips-stage").w_full().grow(1.0).gap(16.0)
        .child(row().gap(12.0).items_center()
            .child(text("256×256 1 px checkerboard downscaled on one Pixels handle").font(type_::LABEL).color(theme.text_secondary))
            .child(next_btn))
        .child(row().w_full().grow(1.0).gap(16.0)
            .child(mips_card("Pixels::replace (unmipped stream)", &stream_info, &s.streamed, theme))
            .child(mips_card("ImageCache::rgba_into (mipmapped)", &mip_info, &s.mipmapped, theme)))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let theme = tok::theme(true);
    let hint_text = if s.mips_mode { "r next frame    m back to grid" } else { "1–4 pick    ↩ zoom    esc back    t curves    m mips" };
    let hint = text(hint_text).font(type_::LABEL).color(theme.text_secondary);
    let mut bar = el().key("pick-bar").w((s.picked as f32 + 1.0) * 24.0).h(4.0).radius(2.0).fill(tok::accent(0, true));
    if s.timed {
        bar = bar.frame_motion(Curve::tween(0.20, Easing::EASE_OUT));
    }
    let pick_track = el().key("pick-track").w(96.0).h(4.0).radius(2.0).fill(theme.control_off).child(bar);
    let mut mode = row().key("mode").gap(8.0).px(10.0).py(4.0).radius(8.0)
        .fill(theme.card)
        .press(theme.text.alpha(0.12))
        .press_scale(0.97)
        .on_press(|s: &mut State| s.timed = !s.timed)
        .child(text("Spring").font(type_::LABEL).color(if !s.timed { theme.text } else { theme.text_secondary }))
        .child(text("Timed").font(type_::LABEL).color(if s.timed { tok::accent(0, true) } else { theme.text_secondary }));
    if s.timed {
        mode = mode.press_motion(
            Curve::tween(0.12, Easing::EASE_OUT),
            Curve::tween(0.16, Easing::EASE_OUT),
        );
    }
    let header = row().w_full().center_y().justify_between()
        .child(hint)
        .child(row().gap(12.0).center_y().child(pick_track).child(mode));
    let stage = if s.mips_mode {
        mips_view(s, &theme)
    } else {
        let stage = col().key("stage").w_full().grow(1.0);
        if s.zoomed {
            stage.child(tile(s, s.picked))
        } else {
            // The grid's own entrance and exit; the matched tile rides above it.
            let pair = |a: usize| row().key(format!("row-{a}")).grow(1.0).gap(12.0).child(tile(s, a)).child(tile(s, a + 1));
            let mut grid = col().key("grid").grow(1.0).gap(12.0).transition(Transition::scale(0.96))
                .child(pair(0)).child(pair(2));
            if s.timed {
                grid = grid.transition_motion(
                    Curve::tween(0.20, Easing::EASE_OUT),
                    Curve::tween(0.18, Easing::EASE_OUT),
                );
            }
            stage.child(grid)
        }
    };
    col().w_full().h_full().padding(24.0).gap(14.0).fill(theme.bg).child(header).child(stage)
}

fn on_key(s: &mut State, e: &KeyEvent, _: &mut Tx) -> bool {
    match &e.key {
        Key::Text(t) if t.eq_ignore_ascii_case("m") => {
            if s.mips_mode { s.mips_mode = false; } else { s.enter_mips(false); }
        }
        Key::Text(t) if s.mips_mode && t.eq_ignore_ascii_case("r") => s.step_stream(false),
        Key::Text(t) if !s.mips_mode && !s.zoomed && let Some(n @ 1..=4) = t.parse::<usize>().ok() => s.picked = n - 1,
        Key::Text(t) if !s.mips_mode && t.eq_ignore_ascii_case("t") => s.timed = !s.timed,
        Key::Enter if !s.mips_mode => s.zoomed = true,
        Key::Escape if s.mips_mode => s.mips_mode = false,
        Key::Escape => s.zoomed = false,
        _ => return false,
    }
    true
}

fn main() {
    rune::run(AppSpec::<State> {
        setup: Some(setup),
        scenes: &[
            ("grid", |_| {}),
            ("one", |s| s.zoomed = true),
            ("timed", |s| s.timed = true),
            ("mips", |s| s.enter_mips(true)),
        ],
        on_key: Some(on_key),
        ..AppSpec::new("Matched", (720.0, 480.0), init, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn enter_zooms_the_picked_tile_from_its_grid_cell() {
        let mut rt = Runtime::new(init(), view, (720.0, 480.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|s, _| s.picked = 3);
        rt.frame(1.0 / 60.0);
        let cell = rt.presented("tile-4").unwrap().0;
        rt.update(|s, _| s.zoomed = true);
        rt.frame(1.0 / 120.0);
        let first = rt.presented("tile-4").unwrap().0;
        assert!((first[0] - cell[0]).abs() < 8.0 && (first[2] - cell[2]).abs() < 8.0, "{cell:?} -> {first:?}");
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        let one = rt.presented("tile-4").unwrap().0;
        assert!(one[2] > 600.0 && one[3] > 350.0, "fills the stage: {one:?}");
    }

    #[test]
    fn mips_scene_reuses_stream_buffer_reads_back_and_downscales_with_mips() {
        let mut s = init();
        s.enter_mips(true);
        assert!(s.reused >= 1, "replace must recycle the previous frame buffer");
        assert!(s.streamed.with_rgba(|w, h, bytes| w == BOARD_SIDE && h == BOARD_SIDE && bytes.len() == (BOARD_SIDE * BOARD_SIDE * 4) as usize).unwrap());
        assert_eq!(s.mipmapped.size(), (BOARD_SIDE, BOARD_SIDE));
        assert_eq!(s.mipmapped.with_rgba(|_, _, _| ()), None);

        let (streamed, mipmapped) = (s.streamed.clone(), s.mipmapped.clone());
        let mut rt = Runtime::new(s, view, (720.0, 480.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let raw = rt.images.place(&streamed, [37.0, 37.0]).unwrap();
        let mip = rt.images.place(&mipmapped, [37.0, 37.0]).unwrap();
        assert!(!raw.premultiplied && raw.rect[2] == 256);
        assert!(mip.premultiplied && mip.rect[2] == 64);
    }

    #[test]
    fn timed_mode_zooms_and_settles_on_timed_curves() {
        let mut st = init();
        st.timed = true;
        let mut rt = Runtime::new(st, view, (720.0, 480.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|s, _| { s.picked = 1; s.zoomed = true; });
        rt.frame(0.0);
        rt.frame(0.11);
        let mid = rt.presented("tile-2").unwrap().0;
        assert!(mid[2] > 340.0 && mid[2] < 660.0, "mid-flight width: {mid:?}");
        rt.frame(0.20);
        let landed = rt.presented("tile-2").unwrap().0;
        assert!(landed[2] > 600.0 && landed[3] > 350.0, "landed width: {landed:?}");
    }
}
