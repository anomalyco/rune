//! `wasm32-unknown-unknown` browser WebAssembly sandbox using a lazy, terminable Web Worker.
//!
//! - Zero `wasmi` linked into the web bundle: uses the browser's own `WebAssembly.instantiate`
//!   inside a dedicated `Worker` created from an inline `Blob` URL on the first closed math fence.
//! - Hard host limits: 16 KiB input cap, 250 ms `setTimeout` watchdog that calls
//!   `worker.terminate()` on hangs or stalls, `onerror` recovery, and full host `RM01` wire
//!   validation + `skrifa`/`tiny-skia` rasterisation with `<= 4096 × 4096` WebGL2 texture caps.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use js_sys::{Array, ArrayBuffer, Object, Reflect, Uint8Array};
use rune::Color;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Blob, BlobPropertyBag, ErrorEvent, MessageEvent, Url, Worker};

use super::raster;
use super::wire::{self, MAX_WIRE_BYTES};
use super::{MathKey, finish_job};

pub(super) const MAX_INPUT_BYTES: usize = 16 * 1024;
const MAX_WASM_INFLATE_BYTES: usize = 4 * 1024 * 1024;
const DEADLINE_MS: i32 = 250;
const MAX_QUEUED: usize = 64;

static RATEX_WASM_DEFLATE: &[u8] = include_bytes!("../../assets/math/ratex-math.wasm.deflate");

const WORKER_JS: &str = r#"
let inst = null;
self.onmessage = async (ev) => {
  const msg = ev.data;
  if (msg.kind === 'init') {
    try {
      const { instance } = await WebAssembly.instantiate(msg.wasm, {});
      inst = instance;
      self.postMessage({ kind: 'ready' });
    } catch (_) {
      self.postMessage({ kind: 'init_err' });
    }
    return;
  }
  if (msg.kind === 'eval') {
    if (!inst) { self.postMessage({ kind: 'res', id: msg.id, bytes: null }); return; }
    try {
      const { alloc_input, render_tex, out_ptr, memory } = inst.exports;
      const input = new Uint8Array(msg.tex);
      const ptr = alloc_input(input.length) >>> 0;
      if (!ptr) { self.postMessage({ kind: 'res', id: msg.id, bytes: null }); return; }
      new Uint8Array(memory.buffer, ptr, input.length).set(input);
      const len = render_tex(input.length) >>> 0;
      if (!len || len > 1048576) { self.postMessage({ kind: 'res', id: msg.id, bytes: null }); return; }
      const op = out_ptr() >>> 0;
      const copy = new Uint8Array(memory.buffer, op, len).slice();
      self.postMessage({ kind: 'res', id: msg.id, bytes: copy.buffer }, [copy.buffer]);
    } catch (_) {
      inst = null;
      self.postMessage({ kind: 'res', id: msg.id, bytes: null });
    }
  }
};
"#;

struct Job {
    id: u32,
    key: MathKey,
    em_pt: f32,
    ink: Color,
    scale: f32,
}

struct WebSandbox {
    wasm_bytes: Option<Vec<u8>>,
    worker: Option<Worker>,
    blob_url: Option<String>,
    ready: bool,
    in_flight: Option<(Job, i32)>,
    queue: VecDeque<Job>,
    next_id: u32,
    _on_msg: Option<Closure<dyn FnMut(MessageEvent)>>,
    _on_err: Option<Closure<dyn FnMut(ErrorEvent)>>,
    _on_timeout: Option<Closure<dyn FnMut()>>,
}

thread_local! {
    static SANDBOX: Rc<RefCell<WebSandbox>> = Rc::new(RefCell::new(WebSandbox {
        wasm_bytes: None,
        worker: None,
        blob_url: None,
        ready: false,
        in_flight: None,
        queue: VecDeque::new(),
        next_id: 1,
        _on_msg: None,
        _on_err: None,
        _on_timeout: None,
    }));
}

fn clear_timer(timer_id: i32) {
    if let Some(win) = web_sys::window() {
        win.clear_timeout_with_handle(timer_id);
    }
}

fn kill_worker(st: &mut WebSandbox) {
    if let Some(w) = st.worker.take() {
        w.terminate();
    }
    if let Some(url) = st.blob_url.take() {
        let _ = Url::revoke_object_url(&url);
    }
    st.ready = false;
    st._on_msg = None;
    st._on_err = None;
    st._on_timeout = None;
}

fn ensure_worker(cell: &Rc<RefCell<WebSandbox>>) -> bool {
    let mut st = cell.borrow_mut();
    if st.worker.is_some() {
        return true;
    }
    let wasm = match &st.wasm_bytes {
        Some(b) => b.clone(),
        None => {
            let Ok(b) = miniz_oxide::inflate::decompress_to_vec_with_limit(
                RATEX_WASM_DEFLATE,
                MAX_WASM_INFLATE_BYTES,
            ) else {
                return false;
            };
            st.wasm_bytes = Some(b.clone());
            b
        }
    };
    let parts = Array::new();
    parts.push(&JsValue::from_str(WORKER_JS));
    let bag = BlobPropertyBag::new();
    bag.set_type("application/javascript");
    let Ok(blob) = Blob::new_with_str_sequence_and_options(&parts, &bag) else {
        return false;
    };
    let Ok(url) = Url::create_object_url_with_blob(&blob) else {
        return false;
    };
    let Ok(worker) = Worker::new(&url) else {
        let _ = Url::revoke_object_url(&url);
        return false;
    };

    let cell_msg = cell.clone();
    let on_msg = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        handle_message(&cell_msg, ev);
    });
    worker.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));

    let cell_err = cell.clone();
    let on_err = Closure::<dyn FnMut(ErrorEvent)>::new(move |_ev: ErrorEvent| {
        handle_failure(&cell_err);
    });
    worker.set_onerror(Some(on_err.as_ref().unchecked_ref()));

    let init = Object::new();
    let _ = Reflect::set(&init, &JsValue::from_str("kind"), &JsValue::from_str("init"));
    let buf = Uint8Array::from(wasm.as_slice()).buffer();
    let _ = Reflect::set(&init, &JsValue::from_str("wasm"), &buf);
    let transfer = Array::new();
    transfer.push(&buf);
    if worker.post_message_with_transfer(&init, &transfer).is_err() {
        worker.terminate();
        let _ = Url::revoke_object_url(&url);
        return false;
    }

    st.worker = Some(worker);
    st.blob_url = Some(url);
    st.ready = false;
    st._on_msg = Some(on_msg);
    st._on_err = Some(on_err);
    true
}

fn pump(cell: &Rc<RefCell<WebSandbox>>) {
    {
        let st = cell.borrow();
        if st.in_flight.is_some() || st.queue.is_empty() {
            return;
        }
    }
    if !ensure_worker(cell) {
        let mut st = cell.borrow_mut();
        while let Some(job) = st.queue.pop_front() {
            finish_job(job.key, None);
        }
        return;
    }
    let mut st = cell.borrow_mut();
    if !st.ready || st.in_flight.is_some() {
        return;
    }
    let Some(job) = st.queue.pop_front() else {
        return;
    };
    let Some(worker) = st.worker.as_ref() else {
        finish_job(job.key, None);
        return;
    };
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("kind"), &JsValue::from_str("eval"));
    let _ = Reflect::set(&msg, &JsValue::from_str("id"), &JsValue::from_f64(f64::from(job.id)));
    let tex_buf = Uint8Array::from(job.key.src.as_bytes()).buffer();
    let _ = Reflect::set(&msg, &JsValue::from_str("tex"), &tex_buf);
    let transfer = Array::new();
    transfer.push(&tex_buf);
    if worker.post_message_with_transfer(&msg, &transfer).is_err() {
        kill_worker(&mut st);
        finish_job(job.key, None);
        return;
    }
    let cell_t = cell.clone();
    let expected_id = job.id;
    let on_timeout = Closure::<dyn FnMut()>::new(move || {
        let timed_out = cell_t
            .borrow()
            .in_flight
            .as_ref()
            .is_some_and(|(j, _)| j.id == expected_id);
        if timed_out {
            handle_failure(&cell_t);
        }
    });
    let timer_id = web_sys::window()
        .and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                on_timeout.as_ref().unchecked_ref(),
                DEADLINE_MS,
            )
            .ok()
        })
        .unwrap_or(0);
    st._on_timeout = Some(on_timeout);
    st.in_flight = Some((job, timer_id));
}

fn handle_message(cell: &Rc<RefCell<WebSandbox>>, ev: MessageEvent) {
    let data = ev.data();
    let kind = Reflect::get(&data, &JsValue::from_str("kind"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    match kind.as_str() {
        "ready" => {
            cell.borrow_mut().ready = true;
            pump(cell);
        }
        "init_err" => {
            let mut st = cell.borrow_mut();
            kill_worker(&mut st);
            while let Some(job) = st.queue.pop_front() {
                finish_job(job.key, None);
            }
        }
        "res" => {
            let id = Reflect::get(&data, &JsValue::from_str("id"))
                .ok()
                .and_then(|v| v.as_f64())
                .map(|n| n as u32)
                .unwrap_or(0);
            let done = {
                let mut st = cell.borrow_mut();
                match st.in_flight.take() {
                    Some((job, tid)) if job.id == id => {
                        clear_timer(tid);
                        st._on_timeout = None;
                        Some(job)
                    }
                    other => {
                        st.in_flight = other;
                        None
                    }
                }
            };
            if let Some(job) = done {
                let bytes = Reflect::get(&data, &JsValue::from_str("bytes"))
                    .ok()
                    .and_then(|v| v.dyn_into::<ArrayBuffer>().ok())
                    .map(|ab| Uint8Array::new(&ab).to_vec())
                    .filter(|b| b.len() <= MAX_WIRE_BYTES);
                let rendered = bytes
                    .as_deref()
                    .and_then(wire::decode)
                    .and_then(|dl| raster::rasterize(&dl, job.em_pt, job.ink, job.scale));
                finish_job(job.key, rendered);
            }
            pump(cell);
        }
        _ => {}
    }
}

fn handle_failure(cell: &Rc<RefCell<WebSandbox>>) {
    let failed = {
        let mut st = cell.borrow_mut();
        kill_worker(&mut st);
        st.in_flight.take().map(|(job, tid)| {
            clear_timer(tid);
            job
        })
    };
    if let Some(job) = failed {
        finish_job(job.key, None);
    }
    pump(cell);
}

pub(super) fn enqueue(key: MathKey, em_pt: f32, ink: Color, scale: f32) -> bool {
    SANDBOX.with(|cell| {
        {
            let mut st = cell.borrow_mut();
            if st.queue.len() >= MAX_QUEUED {
                return false;
            }
            let id = st.next_id;
            st.next_id = st.next_id.wrapping_add(1).max(1);
            st.queue.push_back(Job { id, key, em_pt, ink, scale });
        }
        pump(cell);
        true
    })
}
