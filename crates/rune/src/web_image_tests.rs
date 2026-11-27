use crate::{Align, Color, Cx, El, Env, ImageFilter, Runtime, ScrollSpec, SurfaceFit, col, el, memo::memo};

fn test_env(scale: f32) -> Env {
    Env::new(true, scale)
}

#[test]
fn url_validation_allows_http_https_relative_and_rejects_unsafe_schemes() {
    struct State(&'static str);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().size(200.0, 200.0)
            .child(el().key("img").size(48.0, 48.0).web_image(s.0))
    }

    for ok in [
        "https://cdn.example.com/avatar.png?size=64#v1",
        "http://127.0.0.1:8080/photo.webp",
        "/assets/thumb.jpg",
        "images/logo.png",
    ] {
        let mut rt = Runtime::new(State(ok), view, (200.0, 200.0), test_env(1.0));
        rt.frame(0.0);
        assert_eq!(rt.web_images().len(), 1, "expected accepted URL {ok:?}");
        assert_eq!(rt.web_images()[0].url.as_ref(), ok);
    }

    for bad in [
        "",
        "   ",
        "javascript:alert(1)",
        "mailto:user@example.com",
        "file:///etc/passwd",
        "data:image/png;base64,AAAA",
        "//evil.example.com/x.png",
        "https://cdn.example.com/bad url.png",
        "https://cdn.example.com/<script>.png",
    ] {
        let mut rt = Runtime::new(State(bad), view, (200.0, 200.0), test_env(1.0));
        rt.frame(0.0);
        assert!(rt.web_images().is_empty(), "expected rejected URL {bad:?}");
    }
}

#[test]
fn layout_border_inset_fit_anchor_filter_and_placeholder_fill() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(300.0, 200.0).child(
            el().key("avatar")
                .absolute(20.0, 10.0)
                .size(64.0, 48.0)
                .radius(12.0)
                .fill(Color::WHITE)
                .border(2.0, Color::BLACK)
                .web_image_fit("https://cdn.example.com/a.png", SurfaceFit::Fill)
                .surface_anchor(Align::Start, Align::End)
                .image_filter(ImageFilter::Nearest),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 200.0), test_env(2.0));
    rt.frame(0.0);

    // Canvas still paints the placeholder fill and border, without any GPU image instance.
    assert!(!rt.instances.is_empty());
    assert_eq!(rt.web_images().len(), 1);
    let draw = &rt.web_images()[0];
    assert_eq!(draw.url.as_ref(), "https://cdn.example.com/a.png");
    // Inset by the 2pt border on every side, keeping CSS pixels at 2x DPR.
    assert_eq!(draw.rect, [22.0, 12.0, 60.0, 44.0]);
    assert_eq!(draw.radii, [10.0, 10.0, 10.0, 10.0]);
    assert_eq!(draw.fit, Some(SurfaceFit::Fill));
    assert_eq!(draw.anchor, [Align::Start, Align::End]);
    assert!(!draw.unscaled);
    assert!(draw.nearest);
    assert!(draw.clip.is_none());
    assert!(draw.rounded_clips.is_empty());
    assert!(draw.transform.is_none());

    // A web_image leaf is visible content, not a blank hit zone.
    assert!(!rt.core().tree.blank_leaves().iter().any(|p| p.contains("avatar")));
}

#[test]
fn scroll_viewport_clips_and_culls_web_images_every_frame() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 100.0).scroll(ScrollSpec::default()).child(
            col().w_full().child(
                el().key("top").size(80.0, 60.0).web_image("https://cdn.example.com/1.png"),
            ).child(
                el().key("mid").size(80.0, 60.0).web_image("https://cdn.example.com/2.png"),
            ).child(
                el().key("bot").size(80.0, 60.0).web_image("https://cdn.example.com/3.png"),
            ),
        )
    }

    let mut rt = Runtime::new((), view, (200.0, 100.0), test_env(1.0));
    rt.frame(0.0);
    // At scroll=0: top [0..60] unclipped, mid [60..120] clipped to [60..100], bot [120..180] culled.
    assert_eq!(rt.web_images().len(), 2);
    assert_eq!(rt.web_images()[0].rect, [0.0, 0.0, 80.0, 60.0]);
    assert_eq!(rt.web_images()[0].clip, None);
    let mid_id = rt.web_images()[1].id;
    assert_eq!(rt.web_images()[1].rect, [0.0, 60.0, 80.0, 60.0]);
    assert_eq!(rt.web_images()[1].clip, Some([0.0, 60.0, 80.0, 40.0]));

    // Scroll down 70pt: top [-70..-10] culled, mid [-10..50] clipped at top to [0..50], bot [50..110] clipped at bottom to [50..100].
    rt.wheel(50.0, 50.0, -70.0);
    rt.frame(0.016);
    assert_eq!(rt.web_images().len(), 2);
    assert_eq!(rt.web_images()[0].id, mid_id, "NodeId must remain stable across scroll frames");
    assert_eq!(rt.web_images()[0].url.as_ref(), "https://cdn.example.com/2.png");
    assert_eq!(rt.web_images()[0].rect[1], -10.0);
    assert_eq!(rt.web_images()[0].clip, Some([0.0, 0.0, 80.0, 50.0]));
    assert_eq!(rt.web_images()[1].url.as_ref(), "https://cdn.example.com/3.png");
    assert_eq!(rt.web_images()[1].clip, Some([0.0, 50.0, 80.0, 50.0]));
}

#[test]
fn enclosing_rounded_clips_and_rotation_and_opacity_compose() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(300.0, 300.0).child(
            el().key("card")
                .absolute(10.0, 20.0)
                .size(120.0, 100.0)
                .radius(24.0)
                .clip_rounded(24.0)
                .opacity(0.5)
                .child(
                    el().key("pic")
                        .absolute(0.0, 0.0)
                        .size(120.0, 100.0)
                        .layer_opacity(0.5)
                        .web_image("https://cdn.example.com/card.jpg"),
                ),
        ).child(
            el().key("rot")
                .absolute(150.0, 40.0)
                .size(40.0, 20.0)
                .rotate(std::f32::consts::FRAC_PI_2)
                .web_image("https://cdn.example.com/rot.png"),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 300.0), test_env(1.0));
    rt.frame(0.0);
    assert_eq!(rt.web_images().len(), 2);

    let card_pic = &rt.web_images()[0];
    assert_eq!(card_pic.rounded_clips.len(), 1);
    assert_eq!(card_pic.rounded_clips[0].rect, [10.0, 20.0, 120.0, 100.0]);
    assert_eq!(card_pic.rounded_clips[0].radii, [24.0; 4]);
    assert!((card_pic.opacity - 0.25).abs() < 1e-3, "got {}", card_pic.opacity);

    let rot_pic = &rt.web_images()[1];
    let m = rot_pic.transform.expect("rotated image must emit CSS matrix");
    // 90 deg clockwise: cos=0, sin=1 -> CSS matrix(0, 1, -1, 0, e, f).
    assert!(m[0].abs() < 1e-4 && (m[1] - 1.0).abs() < 1e-4 && (m[2] + 1.0).abs() < 1e-4 && m[3].abs() < 1e-4);
    // Top-left (150, 40) rotated 90 deg CW around centre (170, 50) lands at (180, 30).
    assert!((m[4] - 180.0).abs() < 0.05 && (m[5] - 30.0).abs() < 0.05, "got {m:?}");
}

#[test]
fn zoom_and_web_document_page_offset_convert_to_css_pixels() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        cx.zoom(1.5);
        el().w(400.0).h(1200.0).child(
            el().key("hero").absolute(10.0, 200.0).size(100.0, 50.0).web_image("https://cdn.example.com/hero.png"),
        )
    }

    let mut rt = Runtime::new((), view, (400.0, 600.0), test_env(2.0));
    rt.core_mut().page_mode();
    rt.core_mut().set_band(600.0, 400.0);
    rt.frame(0.0);

    // At 1.5x zoom (window_scale=2.0, env.scale=3.0): view coords * 1.5 = CSS pixels.
    assert_eq!(rt.web_images().len(), 1);
    assert_eq!(rt.web_images()[0].rect, [15.0, 300.0, 150.0, 75.0]);
}

#[test]
fn paint_keep_and_cache_layer_preserve_web_images_and_memo_updates_url() {
    struct State {
        url: &'static str,
        tick: u32,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let u = s.url;
        el().size(200.0, 200.0)
            .child(
                el().key("cached").size(100.0, 100.0).cache().child(
                    memo("pic-memo", u, move || {
                        el().key("pic").size(60.0, 40.0).web_image(u)
                    }),
                ),
            )
            .child(el().key("sibling").absolute(120.0, s.tick as f32).size(10.0, 10.0).fill(Color::WHITE))
    }

    let mut rt = Runtime::new(State { url: "https://cdn.example.com/v1.png", tick: 0 }, view, (200.0, 200.0), test_env(1.0));
    rt.frame(0.0);
    assert_eq!(rt.web_images().len(), 1);
    assert_eq!(rt.web_images()[0].url.as_ref(), "https://cdn.example.com/v1.png");

    // Moving an unrelated sibling must not drop the cached/kept web image.
    rt.state.tick = 10;
    rt.invalidate();
    rt.frame(0.016);
    assert_eq!(rt.web_images().len(), 1);
    assert_eq!(rt.web_images()[0].url.as_ref(), "https://cdn.example.com/v1.png");

    // Updating the URL invalidates the memoized subtree and updates WebImageDraw.
    rt.state.url = "https://cdn.example.com/v2.png";
    rt.invalidate();
    rt.frame(0.016);
    assert_eq!(rt.web_images().len(), 1);
    assert_eq!(rt.web_images()[0].url.as_ref(), "https://cdn.example.com/v2.png");
}

#[test]
fn html_export_and_bridge_json_emit_expected_attributes() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(200.0, 200.0).child(
            el().key("thumb")
                .size(80.0, 60.0)
                .label("Remote photo")
                .web_image_fit("https://cdn.example.com/photo.jpg", SurfaceFit::Fit)
                .surface_anchor(Align::End, Align::Start)
                .image_filter(ImageFilter::Nearest),
        )
    }

    let mut rt = Runtime::new((), view, (200.0, 200.0), test_env(1.0));
    rt.frame(0.0);

    // Static HTML export writes an <img> with src, alt, object-fit, object-position, image-rendering.
    let nodes = rt.core().export_nodes();
    let img = nodes.iter().find(|n| n.tag == "img").expect("exported <img>");
    assert!(img.attrs.contains("src=\"https://cdn.example.com/photo.jpg\""), "{}", img.attrs);
    assert!(img.attrs.contains("alt=\"Remote photo\""), "{}", img.attrs);
    assert!(img.decls.contains(&("object-fit", "contain".into())), "{:?}", img.decls);
    assert!(img.decls.contains(&("object-position", "100% 0%".into())), "{:?}", img.decls);
    assert!(img.decls.contains(&("image-rendering", "pixelated".into())), "{:?}", img.decls);

    // Bridge JSON omits defaults and encodes non-defaults cleanly.
    let raw = super::to_json(rt.web_images());
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let item = &parsed.as_array().unwrap()[0];
    assert_eq!(item["u"], "https://cdn.example.com/photo.jpg");
    assert_eq!(item["f"], "contain");
    assert_eq!(item["p"], "100% 0%");
    assert_eq!(item["n"], true);
    assert!(item.get("o").is_none(), "opaque image should omit opacity");
    assert!(item.get("c").is_none(), "unclipped image should omit clip");
}
