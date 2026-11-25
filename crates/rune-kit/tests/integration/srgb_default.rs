//! Default standalone `Runtime` + `Renderer` + `Offscreen` renders `rune-kit` components (including
//! translucent OpenCode theme borders and surfaces) within 1 LSB of the `App` window path.

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{App, AppSpec, Cx, El, Env, Runtime, col, row};
use rune_kit::*;

fn gallery_card(_: &(), cx: &mut Cx<()>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let controls = row()
        .gap(12.0)
        .child(Button::new("Save").variant(ButtonVariant::Contrast).build(k))
        .child(Button::new("Cancel").variant(ButtonVariant::Outline).build(k))
        .child(Button::new("Disabled").variant(ButtonVariant::Contrast).disabled(true).build(k));
    col()
        .w_full()
        .h_full()
        .fill(k.c.bg_base)
        .padding(16.0)
        .gap(12.0)
        .child(controls)
        .child(
            col()
                .padding(12.0)
                .radius(8.0)
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_muted)
                .child(Badge::new("sRGB default").build(k)),
        )
}

#[test]
fn kit_offscreen_default_matches_app_window_within_1_lsb() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (320u32, 128u32);
    for dark in [false, true] {
        let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
        let mut rt = Runtime::with_text((), gallery_card, (w as f32, h as f32), Env::new(dark, 1.0), text);
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(0.0);

        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, w, h);
        renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
        let offscreen = target.read(&gpu.device);

        let dir = std::env::temp_dir().join(format!("rune-kit-srgb-{dark}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("snap.png");
        let theme = if dark { "dark" } else { "light" };
        App::new(AppSpec {
            fonts: rune_theme_opencode::FONTS,
            families: rune_theme_opencode::FAMILIES,
            ..AppSpec::new("kit-srgb", (w as f32, h as f32), || (), gallery_card)
        })
        .args(["--snap", png.to_str().unwrap(), "--scale", "1", "--theme", theme].map(String::from))
        .try_run()
        .unwrap();

        let bytes = std::fs::read(&png).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let img = rune::image::png_rgba8(&bytes).expect("decode snap png");
        assert_eq!(img.dimensions(), (w, h));
        let app_px = img.into_raw();

        assert_eq!(offscreen.len(), app_px.len());
        let max_diff = offscreen.iter().zip(&app_px).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(
            max_diff <= 1,
            "dark={dark}: default standalone offscreen must match App window within 1 LSB, max_diff={max_diff}"
        );
    }
}
