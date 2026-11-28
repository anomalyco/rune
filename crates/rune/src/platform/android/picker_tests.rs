//! Host-independent unit tests for the Android document picker over `FileHandle`.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use crate::file_dialog::{FileDialogOptions, FileHandle, FileHandleProvider};
use crate::frame::clock::FrameClock;
use crate::platform::embed_frame::step_and_flush;
use crate::{Cx, El, Env, Runtime, el};

use super::picker::{
    PickAction, PickAnswer, PickStep, PickedDocument, create_mime_type, deliver_pick,
    fallback_display_name, open_mime_types, take_pick,
};

#[derive(Default)]
struct Store {
    docs: Mutex<HashMap<String, Vec<u8>>>,
}

struct MemoryDoc {
    store: Arc<Store>,
    uri: String,
}

struct MemoryDocWriter {
    store: Arc<Store>,
    uri: String,
}

impl Write for MemoryDocWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.store.docs.lock().unwrap();
        guard.entry(self.uri.clone()).or_default().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl FileHandleProvider for MemoryDoc {
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        let guard = self.store.docs.lock().unwrap();
        let bytes = guard
            .get(&self.uri)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing test document"))?;
        Ok(Box::new(io::Cursor::new(bytes)))
    }

    fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        let mut guard = self.store.docs.lock().unwrap();
        guard.insert(self.uri.clone(), Vec::new());
        Ok(Box::new(MemoryDocWriter {
            store: Arc::clone(&self.store),
            uri: self.uri.clone(),
        }))
    }
}

#[derive(Default)]
struct AppState {
    single: Vec<Result<Option<FileHandle>, String>>,
    many: Vec<Result<Option<Vec<FileHandle>>, String>>,
    paths: Vec<Result<Option<std::path::PathBuf>, String>>,
    many_paths: Vec<Result<Option<Vec<std::path::PathBuf>>, String>>,
}

fn make_rt() -> Runtime<AppState> {
    let view = |_: &AppState, _: &mut Cx<AppState>| -> El { el() };
    let mut rt = Runtime::new(AppState::default(), view, (360.0, 800.0), Env::default());
    rt.core_mut().host.file_dialogs = true;
    rt
}

#[test]
fn pick_file_starts_open_document_and_delivers_readable_provider_handle() {
    let store = Arc::new(Store::default());
    store.docs.lock().unwrap().insert(
        "content://docs/1".into(),
        b"hello from android saf".to_vec(),
    );
    let mut rt = make_rt();
    rt.update(|_, tx| {
        let opts = FileDialogOptions::default().filter("Images", &["png"]);
        tx.pick_file(opts, |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });

    let PickStep::Start(req) = take_pick(rt.core_mut()) else {
        panic!("expected PickStep::Start");
    };
    assert_eq!(req.action, PickAction::Open { multiple: false });
    assert_eq!(req.mime_type, "image/png");
    assert!(req.extra_mime_types.is_empty());

    // While showing, polling again must not re-issue or refuse the dialog.
    assert_eq!(take_pick(rt.core_mut()), PickStep::None);

    let provider_store = Arc::clone(&store);
    deliver_pick(
        rt.core_mut(),
        req.id,
        PickAnswer::Picked(vec![PickedDocument {
            uri: "content://docs/1".into(),
            name: "photo.png".into(),
        }]),
        move |uri| MemoryDoc { store: Arc::clone(&provider_store), uri },
    );
    assert!(rt.has_pending_input());
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.single.len(), 1);
    let handle = rt.state.single[0].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(handle.name(), "photo.png");
    assert_eq!(handle.path(), None);
    assert_eq!(handle.read_bytes().unwrap(), b"hello from android saf");
}

#[test]
fn pick_files_starts_multi_open_and_delivers_all_handles() {
    let store = Arc::new(Store::default());
    {
        let mut docs = store.docs.lock().unwrap();
        docs.insert("content://docs/a".into(), vec![b'a'; 22]);
        docs.insert("content://docs/b".into(), vec![b'b'; 5008]);
    }
    let mut rt = make_rt();
    rt.update(|_, tx| {
        let opts = FileDialogOptions::default().filter("Data", &["csv", "json"]);
        tx.pick_files(opts, |s: &mut AppState, res, _| {
            s.many.push(res.map_err(|e| e.0));
        });
    });

    let PickStep::Start(req) = take_pick(rt.core_mut()) else {
        panic!("expected PickStep::Start");
    };
    assert_eq!(req.action, PickAction::Open { multiple: true });
    assert_eq!(req.mime_type, "*/*");
    assert_eq!(req.extra_mime_types, ["text/csv", "application/json"]);

    let provider_store = Arc::clone(&store);
    deliver_pick(
        rt.core_mut(),
        req.id,
        PickAnswer::Picked(vec![
            PickedDocument { uri: "content://docs/a".into(), name: "a.csv".into() },
            PickedDocument { uri: "content://docs/b".into(), name: "b.json".into() },
        ]),
        move |uri| MemoryDoc { store: Arc::clone(&provider_store), uri },
    );
    rt.frame(1.0 / 60.0);

    let handles = rt.state.many[0].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(handles.len(), 2);
    assert_eq!(handles[0].name(), "a.csv");
    assert_eq!(handles[0].path(), None);
    assert_eq!(handles[0].read_bytes().unwrap().len(), 22);
    assert_eq!(handles[1].name(), "b.json");
    assert_eq!(handles[1].path(), None);
    assert_eq!(handles[1].read_bytes().unwrap().len(), 5008);
}

#[test]
fn pick_save_file_starts_create_document_and_delivers_writable_handle() {
    let store = Arc::new(Store::default());
    let mut rt = make_rt();
    rt.update(|_, tx| {
        let opts = FileDialogOptions {
            file_name: Some("note.txt".into()),
            ..Default::default()
        };
        tx.pick_save_file(opts, |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });

    let PickStep::Start(req) = take_pick(rt.core_mut()) else {
        panic!("expected PickStep::Start");
    };
    assert_eq!(req.action, PickAction::Create { file_name: "note.txt".into() });
    assert_eq!(req.mime_type, "text/plain");
    assert!(req.extra_mime_types.is_empty());

    let provider_store = Arc::clone(&store);
    deliver_pick(
        rt.core_mut(),
        req.id,
        PickAnswer::Picked(vec![PickedDocument {
            uri: "content://docs/created".into(),
            name: "note.txt".into(),
        }]),
        move |uri| MemoryDoc { store: Arc::clone(&provider_store), uri },
    );
    rt.frame(1.0 / 60.0);

    let handle = rt.state.single[0].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(handle.name(), "note.txt");
    assert_eq!(handle.path(), None);
    let payload = b"saved through android document handle";
    handle.write_bytes(payload).unwrap();
    assert_eq!(store.docs.lock().unwrap().get("content://docs/created").unwrap().as_slice(), payload);
    assert_eq!(handle.read_bytes().unwrap().as_slice(), payload);
}

#[test]
fn cancel_empty_selection_and_error_deliver_expected_outcomes() {
    let store = Arc::new(Store::default());
    let mut rt = make_rt();

    // Cancel
    rt.update(|_, tx| {
        tx.pick_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });
    let PickStep::Start(req) = take_pick(rt.core_mut()) else { panic!() };
    let s1 = Arc::clone(&store);
    deliver_pick(rt.core_mut(), req.id, PickAnswer::Cancelled, move |uri| MemoryDoc {
        store: Arc::clone(&s1),
        uri,
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.single.pop().unwrap().unwrap().is_none());

    // Empty picked list is treated as cancellation (None).
    rt.update(|_, tx| {
        tx.pick_files(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.many.push(res.map_err(|e| e.0));
        });
    });
    let PickStep::Start(req) = take_pick(rt.core_mut()) else { panic!() };
    let s2 = Arc::clone(&store);
    deliver_pick(rt.core_mut(), req.id, PickAnswer::Picked(Vec::new()), move |uri| MemoryDoc {
        store: Arc::clone(&s2),
        uri,
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.many.pop().unwrap().unwrap().is_none());

    // Failure delivers Err.
    rt.update(|_, tx| {
        tx.pick_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });
    let PickStep::Start(req) = take_pick(rt.core_mut()) else { panic!() };
    let s3 = Arc::clone(&store);
    deliver_pick(
        rt.core_mut(),
        req.id,
        PickAnswer::Error("no document picker activity".into()),
        move |uri| MemoryDoc { store: Arc::clone(&s3), uri },
    );
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.single.pop().unwrap().unwrap_err(), "no document picker activity");
}

#[test]
fn path_dialogs_and_open_folder_are_refused_with_errors() {
    let mut rt = make_rt();

    // open_file (path)
    rt.update(|_, tx| {
        tx.open_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.paths.push(res.map_err(|e| e.0));
        });
    });
    assert_eq!(take_pick(rt.core_mut()), PickStep::Refused);
    assert!(rt.has_pending_input());
    rt.frame(1.0 / 60.0);
    assert!(rt.state.paths.pop().unwrap().is_err());

    // open_files (paths)
    rt.update(|_, tx| {
        tx.open_files(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.many_paths.push(res.map_err(|e| e.0));
        });
    });
    assert_eq!(take_pick(rt.core_mut()), PickStep::Refused);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.many_paths.pop().unwrap().is_err());

    // save_file (path)
    rt.update(|_, tx| {
        tx.save_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.paths.push(res.map_err(|e| e.0));
        });
    });
    assert_eq!(take_pick(rt.core_mut()), PickStep::Refused);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.paths.pop().unwrap().is_err());

    // open_folder
    rt.update(|_, tx| {
        tx.open_folder(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.paths.push(res.map_err(|e| e.0));
        });
    });
    assert_eq!(take_pick(rt.core_mut()), PickStep::Refused);
    rt.frame(1.0 / 60.0);
    let err = rt.state.paths.pop().unwrap().unwrap_err();
    assert!(err.contains("folder"), "expected folder error, got: {err}");
}

#[test]
fn stale_request_id_does_not_close_or_answer_newer_dialog() {
    let store = Arc::new(Store::default());
    store.docs.lock().unwrap().insert("content://docs/new".into(), b"fresh".to_vec());
    let mut rt = make_rt();

    rt.update(|_, tx| {
        tx.pick_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });
    let PickStep::Start(first) = take_pick(rt.core_mut()) else { panic!() };
    let s1 = Arc::clone(&store);
    deliver_pick(rt.core_mut(), first.id, PickAnswer::Cancelled, move |uri| MemoryDoc {
        store: Arc::clone(&s1),
        uri,
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.single.len(), 1);

    // Start a second dialog.
    rt.update(|_, tx| {
        tx.pick_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });
    let PickStep::Start(second) = take_pick(rt.core_mut()) else { panic!() };
    assert_ne!(first.id, second.id);

    // A late answer for `first.id` must be ignored and leave `second` open.
    let s2 = Arc::clone(&store);
    deliver_pick(
        rt.core_mut(),
        first.id,
        PickAnswer::Error("stale error".into()),
        move |uri| MemoryDoc { store: Arc::clone(&s2), uri },
    );
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.single.len(), 1);
    assert!(rt.file_dialog().is_some());

    // Answering `second.id` still succeeds.
    let s3 = Arc::clone(&store);
    deliver_pick(
        rt.core_mut(),
        second.id,
        PickAnswer::Picked(vec![PickedDocument {
            uri: "content://docs/new".into(),
            name: "fresh.txt".into(),
        }]),
        move |uri| MemoryDoc { store: Arc::clone(&s3), uri },
    );
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.single.len(), 2);
    let handle = rt.state.single[1].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(handle.read_bytes().unwrap(), b"fresh");
}

#[test]
fn mime_mapping_covers_single_multi_wildcards_unknown_and_save_fallbacks() {
    assert_eq!(open_mime_types(&[]), ("*/*".into(), Vec::new()));

    let single = vec![("PNG".into(), vec!["*.PNG".into()])];
    assert_eq!(open_mime_types(&single), ("image/png".into(), Vec::new()));

    let multi = vec![
        ("Images".into(), vec![".jpg".into(), "jpeg".into(), "webp".into()]),
        ("Docs".into(), vec!["pdf".into()]),
    ];
    assert_eq!(
        open_mime_types(&multi),
        ("*/*".into(), vec!["image/jpeg".into(), "image/webp".into(), "application/pdf".into()]),
    );

    // Wildcard or unknown extension relaxes filter to */* so DocumentsUI never hides valid files.
    let with_wildcard = vec![
        ("Text".into(), vec!["txt".into()]),
        ("All".into(), vec!["*".into()]),
    ];
    assert_eq!(open_mime_types(&with_wildcard), ("*/*".into(), Vec::new()));

    let with_unknown = vec![("Custom".into(), vec!["json".into(), "runeproj".into()])];
    assert_eq!(open_mime_types(&with_unknown), ("*/*".into(), Vec::new()));

    let passthrough = vec![("Any image".into(), vec!["image/*".into()])];
    assert_eq!(open_mime_types(&passthrough), ("image/*".into(), Vec::new()));

    // Save MIME: file_name extension > first concrete filter > application/octet-stream.
    assert_eq!(create_mime_type("export.csv", &[]), "text/csv");
    assert_eq!(create_mime_type("untitled", &[("JSON".into(), vec!["json".into()])]), "application/json");
    assert_eq!(create_mime_type("raw.unknown", &[("Any image".into(), vec!["image/*".into()])]), "application/octet-stream");
    assert_eq!(create_mime_type("", &[]), "application/octet-stream");
}

#[test]
fn fallback_display_name_decodes_saf_uris_and_handles_empty_names() {
    assert_eq!(
        fallback_display_name("content://com.android.externalstorage.documents/document/primary%3ADownloads%2Fmy%20note.txt"),
        "my note.txt",
    );
    assert_eq!(
        fallback_display_name("content://provider/document/primary%3A%E5%BD%95%E9%9F%B3.txt?foo=1"),
        "录音.txt",
    );
    assert_eq!(fallback_display_name("content://provider/"), "document");

    // Empty name in PickedDocument falls back to decoded URI name.
    let store = Arc::new(Store::default());
    let mut rt = make_rt();
    rt.update(|_, tx| {
        tx.pick_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });
    let PickStep::Start(req) = take_pick(rt.core_mut()) else { panic!() };
    deliver_pick(
        rt.core_mut(),
        req.id,
        PickAnswer::Picked(vec![PickedDocument {
            uri: "content://com.android.providers.downloads.documents/document/primary%3ADownloads%2Freport.pdf".into(),
            name: String::new(),
        }]),
        move |uri| MemoryDoc { store: Arc::clone(&store), uri },
    );
    rt.frame(1.0 / 60.0);
    let handle = rt.state.single[0].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(handle.name(), "report.pdf");
}

#[test]
fn embed_frame_keeps_handle_dialog_pending_for_android_picker() {
    let start = crate::time::now();
    let mut rt = make_rt();
    rt.wall_start = Some(start);
    let mut clock = FrameClock::new(start);

    rt.update(|_, tx| {
        tx.pick_file(FileDialogOptions::default(), |s: &mut AppState, res, _| {
            s.single.push(res.map_err(|e| e.0));
        });
    });
    {
        let (core, mut app) = rt.split_mut();
        step_and_flush(&mut clock, core, &mut app, start, false);
    }
    // Because host.file_dialogs is true, step_and_flush left the dialog pending for take_pick.
    assert!(matches!(take_pick(rt.core_mut()), PickStep::Start(_)));
}
