//! Static exit samples on a real native overlay, for capsule/filter integration review.

#![forbid(unsafe_code)]

use rune::{App, AppSpec, Color, Cx, DisplayTarget, El, OverlayInput, OverlayLevel,
    OverlayOptions, OverlaySpec, RunOptions, WindowSpec, el, text};

const INK: Color = Color { r: 0.06, g: 0.32, b: 1.0, a: 1.0 };
const SURFACE: Color = Color { r: 0.035, g: 0.035, b: 0.035, a: 1.0 };

fn view(_: &(), _: &mut Cx<()>) -> El {
    let mut root = el().size(620.0, 176.0);
    for (label, y, clipped) in [("Unmasked", 32.0, false), ("Clipped", 112.0, true)] {
        let mut capsule = el().size(400.0, 40.0).absolute(112.0, y).radius(20.0).fill(SURFACE);
        if clipped { capsule = capsule.clip_rounded(20.0); }
        // Shared background stays outside the dissolve; blur is on the leaving state.
        let state = el().size(72.0, 28.0).absolute(370.0, 6.0).opacity(0.65).blur(1.5)
            .radius(8.0).fill(INK).child(text("Leaving").absolute(8.0, 6.0));
        root = root.child(text(label).absolute(12.0, y + 11.0))
            .child(capsule.child(el().size(400.0, 40.0).dissolve().child(state)));
    }
    root
}

fn main() {
    let spec = AppSpec { title: "Layer filter overlay", size: (620.0, 176.0), min_size: None,
        init: || (), view, scenes: &[], fonts: &[], families: ("sans-serif", "monospace"),
        setup: None, poll: None, on_key: None };
    rune::run_windows_with(RunOptions { accessory: true }, |_| {
        let mut app = App::new(spec).windowless();
        let pill = app.overlay(OverlaySpec::new("Layer filter capsule proof", (620.0, 176.0), view)
            .options(OverlayOptions { display: DisplayTarget::Primary, input: OverlayInput::None,
                level: OverlayLevel::AboveOverlays, exclude_from_capture: false, prewarm: true, ..Default::default() })
            .on_created(|created| eprintln!("filter-overlay created {created:?}")), ());
        pill.show();
        let handle = app.handle();
        // The probe owns stdin and closes it even on capture failure. No persistent GUI job.
        std::thread::spawn(move || {
            std::io::stdin().read_line(&mut String::new()).ok();
            handle.quit();
        });
        vec![WindowSpec::from_app("filter-proof", app)]
    });
}
