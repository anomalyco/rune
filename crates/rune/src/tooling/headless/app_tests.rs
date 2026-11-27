use std::path::PathBuf;
use std::sync::Arc;

use super::{HeadlessApp, TypedHeadlessApp};
use crate::el::el;
use crate::runtime::{Cx, Env, Runtime};
use crate::{App, AppSpec};

fn make_app(with_hook: bool) -> TypedHeadlessApp<Vec<Vec<PathBuf>>> {
    let spec = AppSpec::new("open-files", (100.0, 100.0), Vec::new, |_: &Vec<Vec<PathBuf>>, _: &mut Cx<Vec<Vec<PathBuf>>>| el());
    let mut app = App::new(spec);
    if with_hook {
        app = app.on_open_files(|seen, paths, _| seen.push(paths.to_vec()));
    }
    let (on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files) = (
        app.on_close_request, app.on_quit_request, app.on_close, app.on_quit, app.on_reopen, app.on_open_url, app.on_open_files,
    );
    let (spec, state, shared, _) = app.into_parts(None);
    let rt = Runtime::new(state, spec.view, spec.size, Env::default());
    TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files, settle_ready: None }
}

#[test]
fn headless_run_open_files_delivers_batches_in_order_and_skips_empty() {
    let mut app = make_app(true);
    app.rt.frame(0.0);
    app.run_open_files(&[]);
    assert!(app.rt.state.is_empty(), "empty open_files batch must not call the hook");
    assert!(!app.rt.needs_commit(), "empty open_files batch must not ask for a commit");

    app.run_open_files(&[PathBuf::from("a.torrent"), PathBuf::from("b.torrent")]);
    app.run_open_files(&[PathBuf::from("c.torrent")]);
    assert!(app.rt.needs_commit(), "non-empty open_files commits and redraws");
    assert_eq!(app.rt.state, [
        vec![PathBuf::from("a.torrent"), PathBuf::from("b.torrent")],
        vec![PathBuf::from("c.torrent")],
    ]);
}

#[test]
fn headless_run_open_files_is_a_noop_without_a_hook() {
    let mut app = make_app(false);
    app.rt.frame(0.0);
    app.run_open_files(&[PathBuf::from("a.torrent")]);
    assert!(app.rt.state.is_empty());
    assert!(!app.rt.needs_commit(), "an app without on_open_files is unaffected");
}

#[test]
fn headless_run_close_request_defers_commits_dialog_and_closes_when_allowed() {
    #[derive(Default)]
    struct Draft { dirty: bool, prompt: bool, closes: u32 }
    let make = |dirty: bool, with_hook: bool| {
        let spec = AppSpec::new("draft", (100.0, 100.0), Draft::default, |_: &Draft, _: &mut Cx<Draft>| el());
        let mut app = App::new(spec)
            .init(move || Draft { dirty, prompt: false, closes: 0 })
            .on_close(|d, _| d.closes += 1);
        if with_hook {
            app = app.on_close_request(|d, _| {
                if d.dirty {
                    d.prompt = true;
                    crate::CloseResponse::Defer
                } else {
                    crate::CloseResponse::Close
                }
            });
        }
        let (on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files) = (
            app.on_close_request, app.on_quit_request, app.on_close, app.on_quit, app.on_reopen, app.on_open_url, app.on_open_files,
        );
        let (spec, state, shared, _) = app.into_parts(None);
        let rt = Runtime::new(state, spec.view, spec.size, Env::default());
        TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files, settle_ready: None }
    };

    let mut deferred = make(true, true);
    deferred.rt.frame(0.0);
    assert_eq!(deferred.run_close_request(), crate::CloseResponse::Defer);
    assert!(deferred.rt.state.prompt);
    assert_eq!(deferred.rt.state.closes, 0, "Defer must not invoke on_close");
    assert!(deferred.rt.needs_commit());

    let mut allowed = make(false, true);
    allowed.rt.frame(0.0);
    assert_eq!(allowed.run_close_request(), crate::CloseResponse::Close);
    assert!(!allowed.rt.state.prompt);
    assert_eq!(allowed.rt.state.closes, 1);

    let mut plain = make(true, false);
    plain.rt.frame(0.0);
    assert_eq!(plain.run_close_request(), crate::CloseResponse::Close);
    assert_eq!(plain.rt.state.closes, 1);
}

#[test]
fn headless_run_quit_request_defers_commits_dialog_and_quits_when_allowed() {
    #[derive(Default)]
    struct Draft { dirty: bool, prompt: bool, quits: u32 }
    let make = |dirty: bool, with_hook: bool| {
        let spec = AppSpec::new("draft", (100.0, 100.0), Draft::default, |_: &Draft, _: &mut Cx<Draft>| el());
        let mut app = App::new(spec)
            .init(move || Draft { dirty, prompt: false, quits: 0 })
            .on_quit(|d, _| d.quits += 1);
        if with_hook {
            app = app.on_quit_request(|d, _| {
                if d.dirty {
                    d.prompt = true;
                    crate::QuitResponse::Defer
                } else {
                    crate::QuitResponse::Quit
                }
            });
        }
        let (on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files) = (
            app.on_close_request, app.on_quit_request, app.on_close, app.on_quit, app.on_reopen, app.on_open_url, app.on_open_files,
        );
        let (spec, state, shared, _) = app.into_parts(None);
        let rt = Runtime::new(state, spec.view, spec.size, Env::default());
        TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files, settle_ready: None }
    };

    let mut deferred = make(true, true);
    deferred.rt.frame(0.0);
    assert_eq!(deferred.run_quit_request(), crate::QuitResponse::Defer);
    assert!(deferred.rt.state.prompt);
    assert_eq!(deferred.rt.state.quits, 0, "Defer must not invoke on_quit");
    assert!(deferred.rt.needs_commit());

    let mut allowed = make(false, true);
    allowed.rt.frame(0.0);
    assert_eq!(allowed.run_quit_request(), crate::QuitResponse::Quit);
    assert!(!allowed.rt.state.prompt);
    assert_eq!(allowed.rt.state.quits, 1);

    let mut plain = make(true, false);
    plain.rt.frame(0.0);
    assert_eq!(plain.run_quit_request(), crate::QuitResponse::Quit);
    assert_eq!(plain.rt.state.quits, 1);
}

#[test]
fn headless_runtime_ignores_user_font_smoothing_and_distinguishes_on_and_off() {
    use crate::{Color, FontSmoothing, text};

    let spec = || {
        AppSpec::new("smoothing", (240.0, 80.0), || (), |_: &(), _: &mut Cx<()>| {
            el().w_full().h_full().fill(crate::color::hex("#141518").unwrap()).child(
                text("Hamburgefonstiv 12345").size(14.0, 400.0).color(Color::WHITE),
            )
        })
    };

    let mut on = App::new(spec()).font_smoothing(FontSmoothing::On).headless_runtime((240.0, 80.0), Env::new(true, 2.0));
    let mut off = App::new(spec()).font_smoothing(FontSmoothing::Off).headless_runtime((240.0, 80.0), Env::new(true, 2.0));
    assert!(!on.core().respect_user_font_smoothing());
    assert!(!off.core().respect_user_font_smoothing());
    assert_eq!(on.font_smoothing(), FontSmoothing::On);
    assert_eq!(off.font_smoothing(), FontSmoothing::Off);
    assert_eq!(on.text.apple_pipeline(), crate::text::ApplePipeline::SMOOTHED);
    assert_eq!(off.text.apple_pipeline(), crate::text::ApplePipeline::UNSMOOTHED);

    on.frame(0.0);
    off.frame(0.0);
    let ink = |rt: &Runtime<()>| -> u64 {
        rt.instances.iter()
            .filter(|i| i.params[1] == crate::render::KIND_GLYPH)
            .map(|i| {
                let page = rt.text.atlas.page_data(i.params[2] as usize).unwrap();
                let (ux, uy) = (i.uv[0] as u32, i.uv[1] as u32);
                let (w, h) = (i.rect[2] as u32, i.rect[3] as u32);
                let mut sum = 0u64;
                for y in 0..h {
                    for x in 0..w {
                        sum += page[((uy + y) * crate::text::ATLAS_SIZE + ux + x) as usize] as u64;
                    }
                }
                sum
            })
            .sum()
    };
    assert!(ink(&off) < ink(&on), "On must dilate more than Off in headless runtime: off={} on={}", ink(&off), ink(&on));
}

#[test]
fn headless_snap_font_smoothing_on_and_off_differ_and_cli_flag_overrides_app() {
    use crate::{Color, FontSmoothing, text};
    let _turn = crate::tooling::headless_run::test_turn();

    let spec = || {
        AppSpec::new("smoothing-snap", (240.0, 80.0), || (), |_: &(), _: &mut Cx<()>| {
            el().w_full().h_full().fill(crate::color::hex("#141518").unwrap()).child(
                text("Hamburgefonstiv 12345").size(14.0, 400.0).color(Color::WHITE),
            )
        })
    };

    let dir = std::env::temp_dir().join(format!("rune-smoothing-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let app_on = dir.join("app_on.png");
    let app_off = dir.join("app_off.png");
    let cli_on = dir.join("cli_on.png");
    let cli_off = dir.join("cli_off.png");

    let snap = |mode: FontSmoothing, extra: &[&str], out: &PathBuf| {
        let mut argv: Vec<String> = vec![
            "--size".into(), "240x80".into(),
            "--scale".into(), "2".into(),
            "--theme".into(), "dark".into(),
            "--snap".into(), out.to_string_lossy().into_owned(),
        ];
        argv.extend(extra.iter().map(|s| (*s).into()));
        super::super::try_headless(App::new(spec()).font_smoothing(mode), crate::tooling::Args::parse_from(argv))
            .map_err(|e| e.message())
    };

    let res_on = snap(FontSmoothing::On, &[], &app_on);
    if let Err(err) = &res_on {
        if err.contains("adapter") || err.contains("GPU") {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    }
    res_on.expect("snap On");
    snap(FontSmoothing::Off, &[], &app_off).expect("snap Off");
    snap(FontSmoothing::Off, &["--font-smoothing", "on"], &cli_on).expect("snap --font-smoothing on");
    snap(FontSmoothing::On, &["--font-smoothing", "off"], &cli_off).expect("snap --font-smoothing off");

    let bytes_app_on = std::fs::read(&app_on).unwrap();
    let bytes_app_off = std::fs::read(&app_off).unwrap();
    let bytes_cli_on = std::fs::read(&cli_on).unwrap();
    let bytes_cli_off = std::fs::read(&cli_off).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert_ne!(bytes_app_on, bytes_app_off, "FontSmoothing::On and Off must not render identical headless PNGs");
    assert_eq!(bytes_cli_on, bytes_app_on, "--font-smoothing on overrides App::font_smoothing(Off)");
    assert_eq!(bytes_cli_off, bytes_app_off, "--font-smoothing off overrides App::font_smoothing(On)");
}
