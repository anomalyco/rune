//! `AppSpec::setup` lends the window's own `TextSystem` so text measured during setup shares
//! loaded fonts and cached layouts with the first frame, respects `with_families` overrides, and
//! unwinds cleanly on panic.

use std::cell::Cell;

use rune::text::{measure, with_families};
use rune::{App, AppSpec, Env, TextStyle, col, row, text};

const INTER: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
const STYLE: TextStyle = TextStyle::new(14.0, 400.0);
const CODE: &str = "let mono = 42; // iiiiiiiiii";

#[test]
fn app_setup_measures_use_window_text_system_and_stay_warm_for_first_frame() {
    // Run on a dedicated thread so no ambient `with_families` override bypasses `lend`.
    std::thread::spawn(|| {
        const HEADING: &str = "Setup warm heading";

        #[derive(Default)]
        struct SetupState {
            sans_w: f32,
            mono_w: f32,
            view_sans_w: Cell<f32>,
            view_mono_w: Cell<f32>,
        }

        fn view(s: &SetupState, _: &mut rune::Cx<SetupState>) -> rune::El {
            s.view_sans_w.set(measure(HEADING, &[], STYLE, None).0);
            s.view_mono_w.set(measure(CODE, &[], STYLE.mono(), None).0);
            col()
                .child(row().child(text(HEADING).key("heading").font(STYLE)))
                .child(row().child(text(CODE).key("code").font(STYLE.mono())))
        }

        let mut spec = AppSpec::new("setup-measure", (480.0, 200.0), SetupState::default, view);
        spec.fonts = &[INTER, PLEX];
        spec.families = ("Inter", "IBM Plex Mono");
        spec.setup = Some(|s, _| {
            s.sans_w = measure(HEADING, &[], STYLE, None).0;
            s.mono_w = measure(CODE, &[], STYLE.mono(), None).0;
        });

        let mut rt = App::new(spec).headless_runtime((480.0, 200.0), Env::new(true, 1.0));
        let shaped_after_setup = rt.text.shaped;
        assert!(
            shaped_after_setup >= 2,
            "AppSpec::setup must shape directly on the window's TextSystem (shaped={shaped_after_setup})",
        );
        assert!(
            rt.text.cached_layout_count() >= 2,
            "layouts measured in setup must stay cached on the window's TextSystem",
        );

        rt.frame(0.0);
        assert_eq!(
            rt.text.shaped, shaped_after_setup,
            "first frame must reuse the layouts shaped during setup without re-shaping",
        );
        assert_eq!(rt.state.sans_w, rt.state.view_sans_w.get());
        assert_eq!(rt.state.mono_w, rt.state.view_mono_w.get());

        let heading_el = rt.elements().iter().find(|e| e.key.as_deref() == Some("heading")).unwrap().target[2];
        let code_el = rt.elements().iter().find(|e| e.key.as_deref() == Some("code")).unwrap().target[2];
        assert!((rt.state.sans_w - heading_el).abs() < 0.6, "sans setup={} vs el={heading_el}", rt.state.sans_w);
        assert!((rt.state.mono_w - code_el).abs() < 0.6, "mono setup={} vs el={code_el}", rt.state.mono_w);
    })
    .join()
    .unwrap();
}

#[test]
fn app_setup_respects_with_families_override_and_recovers_after_panic() {
    std::thread::spawn(|| {
        #[derive(Default)]
        struct GuardState {
            lent_mono: f32,
            override_mono: f32,
            after_override_mono: f32,
        }

        let mut spec = AppSpec::new("setup-guard", (320.0, 120.0), GuardState::default, |_, _| col());
        spec.fonts = &[INTER, PLEX];
        spec.families = ("Inter", "IBM Plex Mono");
        spec.setup = Some(|s, _| {
            s.lent_mono = measure(CODE, &[], STYLE.mono(), None).0;
            {
                let _override = with_families("Inter", "Inter");
                s.override_mono = measure(CODE, &[], STYLE.mono(), None).0;
            }
            s.after_override_mono = measure(CODE, &[], STYLE.mono(), None).0;
        });

        let rt = App::new(spec).headless_runtime((320.0, 120.0), Env::new(true, 1.0));
        assert!(rt.text.shaped >= 1, "unguarded setup measurements must use the lent window TextSystem");
        assert_ne!(
            rt.state.lent_mono, rt.state.override_mono,
            "with_families inside setup must still override the lent TextSystem",
        );
        assert_eq!(
            rt.state.lent_mono, rt.state.after_override_mono,
            "dropping with_families inside setup must return to the lent window TextSystem",
        );

        // A panic inside setup must unwind cleanly and clear the lent slot.
        let mut panic_spec = AppSpec::new("setup-panic", (320.0, 120.0), || (), |_, _| col());
        panic_spec.fonts = &[INTER, PLEX];
        panic_spec.families = ("Inter", "IBM Plex Mono");
        panic_spec.setup = Some(|_, _| {
            let _ = measure("before panic", &[], STYLE, None);
            panic!("intentional setup panic");
        });
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = App::new(panic_spec).headless_runtime((320.0, 120.0), Env::new(true, 1.0));
        }));
        assert!(panicked.is_err());

        // Subsequent setup measurements still lend the new window's TextSystem cleanly.
        let mut after_spec = AppSpec::new("setup-after-panic", (320.0, 120.0), || 0.0f32, |_, _| col());
        after_spec.fonts = &[INTER, PLEX];
        after_spec.families = ("Inter", "IBM Plex Mono");
        after_spec.setup = Some(|w, _| {
            *w = measure("after panic", &[], STYLE, None).0;
        });
        let after_rt = App::new(after_spec).headless_runtime((320.0, 120.0), Env::new(true, 1.0));
        assert!(after_rt.text.shaped >= 1 && after_rt.state > 30.0);
    })
    .join()
    .unwrap();
}
