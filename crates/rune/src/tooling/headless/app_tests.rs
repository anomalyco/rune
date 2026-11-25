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
    TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files }
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
        TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files }
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
        TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_close_request, on_quit_request, on_close, on_quit, on_reopen, on_open_url, on_open_files }
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
