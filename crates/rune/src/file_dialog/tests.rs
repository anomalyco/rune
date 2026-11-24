use super::*;
#[test]
fn scripted_dialogs_preserve_order_unicode_paths_and_cancellation() {
    let path = PathBuf::from("/fixture/录音.wav");
    let dialogs = FileDialogs::scripted([
        ScriptedFileDialog { kind: FileDialogKind::Open, path: Some(path.clone()) },
        ScriptedFileDialog { kind: FileDialogKind::Save, path: None },
        ScriptedFileDialog { kind: FileDialogKind::Folder, path: Some("/fixture/Projects".into()) },
    ]);
    let options = FileDialogOptions::default().filter("Audio", &["wav", "flac"]);
    assert!(pollster::block_on(dialogs.save(options.clone())).is_err());
    assert_eq!(pollster::block_on(dialogs.open(options.clone())).unwrap(), Some(path));
    assert_eq!(pollster::block_on(dialogs.clone().save(options.clone())).unwrap(), None);
    assert!(pollster::block_on(dialogs.open(options.clone())).is_err(), "a folder fixture doesn't answer an Open");
    assert_eq!(pollster::block_on(dialogs.open_folder(options.clone())).unwrap(), Some("/fixture/Projects".into()));
    assert!(pollster::block_on(dialogs.open(options)).is_err());
}

use crate::{Cx, El, Env, Runtime, el};

#[derive(Default)]
struct Import { answers: Vec<Result<Option<PathBuf>, String>> }

fn import(_: &Import, _: &mut Cx<Import>) -> El {
    el().key("import").size(80.0, 20.0).on_press_tx(|_: &mut Import, tx: &mut Tx| {
        let options = FileDialogOptions { title: "Import".into(), ..Default::default() }.filter("Export", &["json"]);
        tx.open_file(options, |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0)));
    })
}

fn press(rt: &mut Runtime<Import>) {
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_handlers_dialog_waits_open_and_its_answer_runs_on_a_later_frame() {
    let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt);
    let (kind, options) = rt.file_dialog().expect("the press asked for a dialog");
    assert_eq!((kind, options.title.as_str(), options.filters.as_slice()), (FileDialogKind::Open, "Import", &[("Export".to_owned(), vec!["json".to_owned()])][..]));
    assert_eq!(rt.file_dialog_json().unwrap(), serde_json::json!({ "kind": "open", "title": "Import", "filters": [{ "name": "Export", "extensions": ["json"] }] }));
    rt.frame(2.0 / 60.0);
    assert!(rt.state.answers.is_empty() && rt.file_dialog().is_some(), "it stays open across frames");

    rt.resolve_file_dialog(Some("/exports/录音.json".into())).unwrap();
    assert!(rt.file_dialog().is_none() && rt.state.answers.is_empty(), "closed; the answer waits for a frame");
    assert!(rt.has_pending_input(), "so the host draws one");
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.answers, [Ok(Some("/exports/录音.json".into()))]);

    press(&mut rt);
    rt.resolve_file_dialog(None).unwrap();
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.answers[1], Ok(None), "cancelling answers None");
    assert!(rt.resolve_file_dialog(None).is_err(), "nothing is open");
}

#[test]
fn a_folder_dialog_waits_open_as_a_folder_and_answers_with_the_folder() {
    fn view(_: &Import, _: &mut Cx<Import>) -> El {
        el().key("open").size(80.0, 20.0).on_press_tx(|_: &mut Import, tx: &mut Tx| {
            let options = FileDialogOptions { title: "Open Project".into(), directory: Some("/work".into()), ..Default::default() };
            tx.open_folder(options, |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0)));
        })
    }
    let mut rt = Runtime::new(Import::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt);
    assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.title.clone())), Some((FileDialogKind::Folder, "Open Project".into())));
    assert_eq!(rt.file_dialog_json().unwrap(), serde_json::json!({ "kind": "folder", "title": "Open Project", "filters": [], "directory": "/work" }));
    rt.resolve_file_dialog(Some("/work/notes".into())).unwrap();
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.answers, [Ok(Some("/work/notes".into()))]);
}

#[test]
fn a_second_dialog_while_one_is_open_is_refused_and_the_first_stays() {
    let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt);
    press(&mut rt);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.answers, [Err("a file dialog is already open".to_owned())]);
    rt.update(|_, tx| tx.save_file(FileDialogOptions::default(), |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0))));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.answers.len(), 2, "an update's dialog too");
    assert_eq!(rt.file_dialog().map(|(kind, _)| kind), Some(FileDialogKind::Open));
    rt.resolve_file_dialog(Some("a.json".into())).unwrap();
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.answers[2], Ok(Some("a.json".into())));

    rt.update(|_, tx| tx.save_file(FileDialogOptions { file_name: Some("out.json".into()), ..Default::default() }, |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0))));
    assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.file_name.clone())), Some((FileDialogKind::Save, Some("out.json".into()))), "once it closed the next opens");
}

#[test]
fn a_host_shows_the_dialog_once_and_answers_it_by_id() {
    let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt);
    let (id, kind, multiple, _) = rt.take_file_dialog().expect("the host takes it");
    assert_eq!((kind, multiple), (FileDialogKind::Open, false));
    assert!(rt.take_file_dialog().is_none(), "showing, so not taken again");
    rt.answer_file_dialog(id, Err(FileDialogError("no portal".into())));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.answers, [Err("no portal".to_owned())]);

    press(&mut rt);
    rt.refuse_file_dialog("unsupported");
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.answers[1], Err("unsupported".to_owned()), "a host without dialogs refuses it");
}

#[test]
fn an_overlay_refuses_file_dialogs() {
    let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
    rt.host.native_overlay = true;
    rt.frame(0.0);
    press(&mut rt);
    assert!(rt.file_dialog().is_none(), "overlays never leave a dialog pending");
    assert!(rt.keys_queued(), "hidden_input delivers the refusal without waiting for a show");
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.answers, [Err("overlays cannot open file dialogs".to_owned())]);
}

#[test]
fn closing_or_hiding_cancels_open_and_queued_file_dialogs() {
    let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| {
        tx.open_file(FileDialogOptions::default(), |s: &mut Import, result, tx| {
            s.answers.push(result.map_err(|e| e.0));
            tx.save_file(FileDialogOptions::default(), |s: &mut Import, follow, _| {
                s.answers.push(follow.map_err(|e| e.0));
            });
        });
        tx.save_file(FileDialogOptions::default(), |s: &mut Import, result, _| {
            s.answers.push(result.map_err(|e| e.0));
        });
    });
    assert!(rt.file_dialog().is_some());
    rt.cancel_file_dialogs();
    assert!(rt.file_dialog().is_none());
    assert_eq!(
        rt.state.answers,
        [
            Err("a file dialog is already open".to_owned()),
            Ok(None),
            Ok(None),
        ],
        "queued answers, the open dialog, and a dialog asked for in its callback all settle before the host drops"
    );
}

#[test]
fn scripted_files_preserve_order_unicode_paths_and_cancellation() {
    let first = vec![PathBuf::from("/fixture/录音.csv"), PathBuf::from("/fixture/two.csv")];
    let dialogs = FileDialogs::scripted_files([Some(first.clone()), None, Some(vec![])]);
    let options = FileDialogOptions::default().filter("CSV", &["csv"]);
    assert!(pollster::block_on(dialogs.open(options.clone())).is_err(), "multi fixture rejects single open");
    assert!(pollster::block_on(dialogs.save(options.clone())).is_err());
    assert!(pollster::block_on(dialogs.open_folder(options.clone())).is_err());
    assert_eq!(pollster::block_on(dialogs.open_files(options.clone())).unwrap(), Some(first));
    assert_eq!(pollster::block_on(dialogs.clone().open_files(options.clone())).unwrap(), None);
    assert_eq!(pollster::block_on(dialogs.open_files(options.clone())).unwrap(), Some(vec![]));
    assert!(pollster::block_on(dialogs.open_files(options.clone())).is_err(), "exhausted");

    let single = FileDialogs::scripted([ScriptedFileDialog { kind: FileDialogKind::Open, path: Some("/fixture/a.csv".into()) }]);
    assert!(pollster::block_on(single.open_files(options)).is_err(), "single fixture rejects open_files");
}

#[derive(Default)]
struct MultiImport { answers: Vec<Result<Option<Vec<PathBuf>>, String>> }

fn multi_import(_: &MultiImport, _: &mut Cx<MultiImport>) -> El {
    el().key("import").size(80.0, 20.0).on_press_tx(|_: &mut MultiImport, tx: &mut Tx| {
        let options = FileDialogOptions { title: "Import CSVs".into(), ..Default::default() }.filter("CSV", &["csv"]);
        tx.open_files(options, |s: &mut MultiImport, result, _| s.answers.push(result.map_err(|e| e.0)));
    })
}

fn press_multi(rt: &mut Runtime<MultiImport>) {
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);
}

#[test]
fn an_open_files_dialog_picks_several_and_a_cancel_picks_none() {
    let mut rt = Runtime::new(MultiImport::default(), multi_import, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.file_dialog_multiple());
    press_multi(&mut rt);
    let (kind, options) = rt.file_dialog().expect("the press asked for a dialog");
    assert_eq!((kind, rt.file_dialog_multiple(), options.title.as_str()), (FileDialogKind::Open, true, "Import CSVs"));
    assert_eq!(
        rt.file_dialog_json().unwrap(),
        serde_json::json!({ "kind": "open", "multiple": true, "title": "Import CSVs", "filters": [{ "name": "CSV", "extensions": ["csv"] }] })
    );
    rt.frame(2.0 / 60.0);
    assert!(rt.state.answers.is_empty() && rt.file_dialog_multiple(), "it stays open across frames");

    let picked = vec![PathBuf::from("/exports/录音.csv"), PathBuf::from("/exports/two.csv")];
    rt.resolve_file_dialog_files(Some(picked.clone())).unwrap();
    assert!(rt.file_dialog().is_none() && !rt.file_dialog_multiple() && rt.state.answers.is_empty());
    assert!(rt.has_pending_input());
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.answers, [Ok(Some(picked))]);

    press_multi(&mut rt);
    rt.resolve_file_dialog_files(None).unwrap();
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.answers[1], Ok(None), "resolve_file_dialog_files(None) cancels");

    press_multi(&mut rt);
    rt.resolve_file_dialog(None).unwrap();
    rt.frame(7.0 / 60.0);
    assert_eq!(rt.state.answers[2], Ok(None), "resolve_file_dialog(None) cancels a multi-file dialog too");
    assert!(rt.resolve_file_dialog_files(None).is_err(), "nothing is open");
}

#[test]
fn a_dialog_for_one_file_answered_with_several_gets_the_first_and_vice_versa() {
    let mut single = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
    single.frame(0.0);
    press(&mut single);
    assert!(!single.file_dialog_multiple());
    single.resolve_file_dialog_files(Some(vec!["/a.json".into(), "/b.json".into()])).unwrap();
    single.frame(2.0 / 60.0);
    assert_eq!(single.state.answers, [Ok(Some("/a.json".into()))]);

    press(&mut single);
    single.resolve_file_dialog_files(Some(vec![])).unwrap();
    single.frame(4.0 / 60.0);
    assert_eq!(single.state.answers[1], Ok(None), "an empty paths list answers a single-file dialog with None");

    let mut multi = Runtime::new(MultiImport::default(), multi_import, (200.0, 100.0), Env::new(true, 1.0));
    multi.frame(0.0);
    press_multi(&mut multi);
    multi.resolve_file_dialog(Some("/only.csv".into())).unwrap();
    multi.frame(2.0 / 60.0);
    assert_eq!(multi.state.answers, [Ok(Some(vec!["/only.csv".into()]))]);

    press_multi(&mut multi);
    multi.resolve_file_dialog_files(Some(vec![])).unwrap();
    multi.frame(4.0 / 60.0);
    assert_eq!(multi.state.answers[1], Ok(Some(vec![])));
}

#[test]
fn concurrent_host_overlay_and_cancel_paths_work_for_open_files() {
    let mut rt = Runtime::new(MultiImport::default(), multi_import, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press_multi(&mut rt);
    press_multi(&mut rt);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.answers, [Err("a file dialog is already open".to_owned())]);
    assert!(rt.file_dialog_multiple(), "the first multi-file dialog stays open");

    let (id, kind, multiple, _) = rt.take_file_dialog().expect("the host takes it");
    assert_eq!((kind, multiple), (FileDialogKind::Open, true));
    assert!(rt.take_file_dialog().is_none(), "showing, so not taken again");
    rt.answer_file_dialog(id, Err(FileDialogError("no portal".into())));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.answers[1], Err("no portal".to_owned()));

    press_multi(&mut rt);
    rt.refuse_file_dialog("unsupported");
    rt.frame(6.0 / 60.0);
    assert_eq!(rt.state.answers[2], Err("unsupported".to_owned()));

    press_multi(&mut rt);
    rt.cancel_file_dialogs();
    assert!(rt.file_dialog().is_none());
    assert_eq!(rt.state.answers[3], Ok(None));

    let mut ov = Runtime::new(MultiImport::default(), multi_import, (200.0, 100.0), Env::new(true, 1.0));
    ov.host.native_overlay = true;
    ov.frame(0.0);
    press_multi(&mut ov);
    assert!(ov.file_dialog().is_none());
    ov.frame(2.0 / 60.0);
    assert_eq!(ov.state.answers, [Err("overlays cannot open file dialogs".to_owned())]);
}
