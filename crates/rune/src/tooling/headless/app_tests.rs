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
    let (on_reopen, on_open_url, on_open_files) = (app.on_reopen, app.on_open_url, app.on_open_files);
    let (spec, state, shared, _) = app.into_parts(None);
    let rt = Runtime::new(state, spec.view, spec.size, Env::default());
    TypedHeadlessApp { rt, shared: Arc::clone(&shared), comp: None, on_reopen, on_open_url, on_open_files }
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
