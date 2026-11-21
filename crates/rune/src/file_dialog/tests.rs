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
    let (id, kind, _) = rt.take_file_dialog().expect("the host takes it");
    assert_eq!(kind, FileDialogKind::Open);
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
