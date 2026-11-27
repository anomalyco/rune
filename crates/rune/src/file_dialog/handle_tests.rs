use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::*;
use crate::{Cx, El, Env, Runtime, el};

fn temp_path(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("rune-file-handle-{tag}-{}-{n}.bin", std::process::id()))
}

#[test]
fn path_handle_reads_writes_truncates_and_exposes_metadata() {
    let path = temp_path("roundtrip");
    let handle = FileHandle::from_path(&path);
    assert_eq!(handle.path(), Some(path.as_path()));
    assert_eq!(handle.name(), path.file_name().unwrap().to_str().unwrap());

    // Write via streaming writer, then read via streaming reader.
    {
        let mut w = handle.writer().expect("create temp file");
        w.write_all(b"initial longer payload 1234567890").unwrap();
        w.flush().unwrap();
    }
    let mut buf = String::new();
    handle.reader().unwrap().read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "initial longer payload 1234567890");

    // Subsequent writer replaces (truncates) instead of leaving trailing bytes.
    {
        let mut w = handle.writer().unwrap();
        w.write_all(b"short").unwrap();
    }
    assert_eq!(handle.read_bytes().unwrap(), b"short");

    // Async write + read round-trip.
    pollster::block_on(handle.write(b"async-payload")).unwrap();
    assert_eq!(pollster::block_on(handle.read()).unwrap(), b"async-payload");

    // Debug shows name and path, never file contents.
    let dbg = format!("{handle:?}");
    assert!(dbg.contains(handle.name()));
    assert!(!dbg.contains("async-payload"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn memory_handle_has_no_path_and_shares_state_across_clones() {
    let handle = FileHandle::from_bytes("录音.txt", b"hello".to_vec());
    let clone = handle.clone();
    assert_eq!(handle.name(), "录音.txt");
    assert_eq!(handle.path(), None);
    assert_eq!(handle.read_bytes().unwrap(), b"hello");

    // Streaming writer replaces contents and is visible through clones.
    {
        let mut w = clone.writer().unwrap();
        w.write_all(b"hi").unwrap();
    }
    let mut out = Vec::new();
    handle.reader().unwrap().read_to_end(&mut out).unwrap();
    assert_eq!(out, b"hi");

    pollster::block_on(handle.write(b"updated")).unwrap();
    assert_eq!(pollster::block_on(clone.read()).unwrap(), b"updated");

    let dbg = format!("{handle:?}");
    assert!(dbg.contains("录音.txt"));
    assert!(!dbg.contains("updated"));
    assert!(!dbg.contains("path"));
}

struct FakeDocProvider {
    data: Arc<Mutex<Vec<u8>>>,
    writable: bool,
}

struct FakeDocWriter(Arc<Mutex<Vec<u8>>>);
impl Write for FakeDocWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl FileHandleProvider for FakeDocProvider {
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(io::Cursor::new(self.data.lock().unwrap().clone())))
    }
    fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        if !self.writable {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "read-only document"));
        }
        self.data.lock().unwrap().clear();
        Ok(Box::new(FakeDocWriter(Arc::clone(&self.data))))
    }
}

struct ReadOnlyDocProvider(Vec<u8>);
impl FileHandleProvider for ReadOnlyDocProvider {
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(io::Cursor::new(self.0.clone())))
    }
}

#[test]
fn provider_handle_simulates_pathless_document_read_and_write() {
    let backing = Arc::new(Mutex::new(b"content://doc-initial".to_vec()));
    let handle = FileHandle::from_provider("report.pdf", FakeDocProvider { data: Arc::clone(&backing), writable: true });
    assert_eq!(handle.name(), "report.pdf");
    assert!(handle.path().is_none());
    assert_eq!(handle.read_bytes().unwrap(), b"content://doc-initial");

    handle.write_bytes(b"replaced").unwrap();
    assert_eq!(backing.lock().unwrap().as_slice(), b"replaced");
    assert_eq!(pollster::block_on(handle.read()).unwrap(), b"replaced");

    // Default FileHandleProvider::writer returns Unsupported.
    let ro = FileHandle::from_provider("readonly.bin", ReadOnlyDocProvider(b"abc".to_vec()));
    assert_eq!(ro.read_bytes().unwrap(), b"abc");
    let err = ro.writer().err().expect("default writer is read-only");
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);
}

#[derive(Default)]
struct PickerState {
    single: Vec<Result<Option<FileHandle>, String>>,
    many: Vec<Result<Option<Vec<FileHandle>>, String>>,
    paths: Vec<Result<Option<PathBuf>, String>>,
}

fn picker_view(_: &PickerState, _: &mut Cx<PickerState>) -> El {
    el().key("pick").size(80.0, 20.0).on_press_tx(|_: &mut PickerState, tx: &mut Tx| {
        let options = FileDialogOptions { title: "Pick".into(), ..Default::default() }.filter("Text", &["txt"]);
        tx.pick_file(options, |s: &mut PickerState, res, _| s.single.push(res.map_err(|e| e.0)));
    })
}

fn press_picker(rt: &mut Runtime<PickerState>) {
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);
}

#[test]
fn pick_file_and_save_file_work_with_both_path_and_handle_resolvers() {
    let mut rt = Runtime::new(PickerState::default(), picker_view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.file_dialog_handles());

    // 1. Desktop / headless path resolver answers pick_file with a path-backed FileHandle.
    press_picker(&mut rt);
    assert!(rt.file_dialog_handles());
    assert!(!rt.file_dialog_multiple());
    assert_eq!(
        rt.file_dialog_json().unwrap(),
        serde_json::json!({ "kind": "open", "handles": true, "title": "Pick", "filters": [{ "name": "Text", "extensions": ["txt"] }] })
    );
    rt.resolve_file_dialog(Some(PathBuf::from("/docs/notes.txt"))).unwrap();
    assert!(!rt.file_dialog_handles());
    rt.frame(2.0 / 60.0);
    let h = rt.state.single[0].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(h.name(), "notes.txt");
    assert_eq!(h.path(), Some(Path::new("/docs/notes.txt")));

    // 2. Host handle resolver answers pick_file with a pathless document handle.
    press_picker(&mut rt);
    let doc = FileHandle::from_bytes("cloud.txt", b"from-provider".to_vec());
    rt.resolve_file_dialog_handle(Some(doc)).unwrap();
    rt.frame(4.0 / 60.0);
    let h = rt.state.single[1].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(h.name(), "cloud.txt");
    assert_eq!(h.path(), None);
    assert_eq!(h.read_bytes().unwrap(), b"from-provider");

    // 3. Cancellation answers Ok(None).
    press_picker(&mut rt);
    rt.resolve_file_dialog_handle(None).unwrap();
    rt.frame(6.0 / 60.0);
    assert!(rt.state.single[2].as_ref().unwrap().is_none());

    // 4. pick_save_file sets Save kind + handles flag and accepts writable pathless handle.
    rt.update(|_, tx| {
        let opts = FileDialogOptions { file_name: Some("export.json".into()), ..Default::default() };
        tx.pick_save_file(opts, |s: &mut PickerState, res, _| s.single.push(res.map_err(|e| e.0)));
    });
    assert_eq!(rt.file_dialog().map(|(k, o)| (k, o.file_name.clone())), Some((FileDialogKind::Save, Some("export.json".into()))));
    assert!(rt.file_dialog_handles());
    let sink = FileHandle::from_bytes("export.json", Vec::new());
    rt.resolve_file_dialog_handle(Some(sink.clone())).unwrap();
    rt.frame(7.0 / 60.0);
    let saved = rt.state.single[3].as_ref().unwrap().as_ref().unwrap();
    saved.write_bytes(b"{\"ok\":true}").unwrap();
    assert_eq!(sink.read_bytes().unwrap(), b"{\"ok\":true}");
}

#[test]
fn pick_files_delivers_multiple_handles_and_rejects_pathless_handles_on_path_dialogs() {
    let mut rt = Runtime::new(PickerState::default(), picker_view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Multi-file handle dialog.
    rt.update(|_, tx| {
        tx.pick_files(FileDialogOptions::default(), |s: &mut PickerState, res, _| s.many.push(res.map_err(|e| e.0)));
    });
    assert!(rt.file_dialog_handles());
    assert!(rt.file_dialog_multiple());
    assert_eq!(rt.file_dialog_json().unwrap()["handles"], serde_json::json!(true));
    assert_eq!(rt.file_dialog_json().unwrap()["multiple"], serde_json::json!(true));
    rt.resolve_file_dialog_handles(Some(vec![
        FileHandle::from_bytes("a.bin", vec![1; 22]),
        FileHandle::from_bytes("b.bin", vec![2; 64]),
    ])).unwrap();
    rt.frame(1.0 / 60.0);
    let picked = rt.state.many[0].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(picked.len(), 2);
    assert_eq!(picked[0].name(), "a.bin");
    assert_eq!(picked[0].read_bytes().unwrap().len(), 22);
    assert_eq!(picked[1].name(), "b.bin");
    assert_eq!(picked[1].read_bytes().unwrap().len(), 64);

    // Path-only dialog (Tx::open_file) rejects pathless handles instead of silently cancelling.
    rt.update(|_, tx| {
        tx.open_file(FileDialogOptions::default(), |s: &mut PickerState, res, _| s.paths.push(res.map_err(|e| e.0)));
    });
    assert!(!rt.file_dialog_handles());
    assert!(rt.file_dialog_json().unwrap().get("handles").is_none());
    assert!(rt.resolve_file_dialog_handle(Some(FileHandle::from_bytes("mem.txt", b"x".to_vec()))).is_err());
    assert!(rt.file_dialog().is_some(), "rejected handle leaves the path dialog open");
    // A path-backed handle is accepted for a path dialog.
    rt.resolve_file_dialog_handle(Some(FileHandle::from_path("/tmp/ok.txt"))).unwrap();
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.paths, [Ok(Some(PathBuf::from("/tmp/ok.txt")))]);
}

#[test]
fn file_dialogs_async_pick_methods_work_with_scripted_and_handle_fixtures() {
    let path_fixtures = FileDialogs::scripted([
        ScriptedFileDialog { kind: FileDialogKind::Open, path: Some(PathBuf::from("/fixture/a.csv")) },
        ScriptedFileDialog { kind: FileDialogKind::Save, path: None },
    ]);
    let h = pollster::block_on(path_fixtures.pick_file(FileDialogOptions::default())).unwrap().unwrap();
    assert_eq!(h.name(), "a.csv");
    assert_eq!(h.path(), Some(Path::new("/fixture/a.csv")));
    assert!(pollster::block_on(path_fixtures.pick_save_file(FileDialogOptions::default())).unwrap().is_none());

    let handle_fixtures = FileDialogs::scripted_handles([
        (FileDialogKind::Open, true, Some(vec![FileHandle::from_bytes("one.txt", b"1".to_vec()), FileHandle::from_bytes("two.txt", b"2".to_vec())])),
        (FileDialogKind::Save, false, Some(vec![FileHandle::from_bytes("out.txt", Vec::new())])),
    ]);
    let many = pollster::block_on(handle_fixtures.pick_files(FileDialogOptions::default())).unwrap().unwrap();
    assert_eq!(many.len(), 2);
    assert_eq!(many[0].read_bytes().unwrap(), b"1");
    assert_eq!(many[1].read_bytes().unwrap(), b"2");
    let save = pollster::block_on(handle_fixtures.pick_save_file(FileDialogOptions::default())).unwrap().unwrap();
    save.write_bytes(b"saved").unwrap();
    assert_eq!(save.read_bytes().unwrap(), b"saved");
}
