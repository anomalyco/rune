//! Browser file picker bridge for handle-based dialogs (`Tx::pick_file`, `Tx::pick_files`,
//! `Tx::pick_save_file`). Path-based dialogs (`Tx::open_file`, `Tx::open_files`, `Tx::save_file`,
//! `Tx::open_folder`) stay refused on web because browsers expose no local filesystem paths.

use wasm_bindgen::prelude::*;

use crate::file_dialog::{DialogAnswer, FileDialogError, FileDialogKind, FileHandle};
use crate::runtime::RuntimeCore;

#[wasm_bindgen(module = "/src/platform/web/bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = openFilePicker)]
    fn open_file_picker(
        handle: &JsValue,
        id: f64,
        kind: &str,
        multiple: bool,
        filters_json: &str,
        file_name: &str,
    );
}

/// Presents a pending handle-based file dialog through the browser picker, or refuses path/folder
/// dialogs that require local filesystem paths.
pub(super) fn flush(rt: &mut RuntimeCore, bridge_handle: &JsValue) {
    let Some((kind, _)) = rt.file_dialog() else { return };
    if !rt.file_dialog_handles() || kind == FileDialogKind::Folder {
        rt.refuse_file_dialog("native file dialogs are unsupported on web");
        return;
    }
    let Some((id, kind, multiple, options)) = rt.take_file_dialog() else { return };
    let filters: Vec<_> = options
        .filters
        .iter()
        .map(|(name, extensions)| serde_json::json!({ "name": name, "extensions": extensions }))
        .collect();
    let filters_json = serde_json::to_string(&filters).unwrap_or_else(|_| "[]".into());
    let kind_str = match kind {
        FileDialogKind::Open => "open",
        FileDialogKind::Save => "save",
        FileDialogKind::Folder => "folder",
    };
    open_file_picker(
        bridge_handle,
        id as f64,
        kind_str,
        multiple,
        &filters_json,
        options.file_name.as_deref().unwrap_or(""),
    );
}

/// Delivers the browser picker's answer (`kind: "file_dialog"`) back to the runtime.
pub(super) fn handle_event(rt: &mut RuntimeCore, event: &serde_json::Value) -> bool {
    let Some(id) = event["id"].as_u64() else { return false };
    if let Some(err) = event["error"].as_str() {
        rt.answer_file_dialog(id, Err(FileDialogError(err.to_owned())));
        return true;
    }
    if event["cancel"].as_bool() == Some(true) {
        rt.answer_file_dialog(id, Ok(DialogAnswer::Handles(None)));
        return true;
    }
    let Some(files) = event["files"].as_array() else {
        rt.answer_file_dialog(id, Ok(DialogAnswer::Handles(None)));
        return true;
    };
    let handles: Vec<FileHandle> = files
        .iter()
        .filter_map(|f| {
            let file_id = u32::try_from(f["id"].as_u64()?).ok()?;
            if !crate::file_dialog::claim_web_picked_file(file_id) {
                return None;
            }
            let name = f["name"].as_str().unwrap_or_default().to_owned();
            Some(FileHandle::from_web(file_id, name))
        })
        .collect();
    rt.answer_file_dialog(id, Ok(DialogAnswer::Handles(Some(handles))));
    true
}
