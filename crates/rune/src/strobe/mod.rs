//! The strobe bridge (Cargo feature `strobe`). With `STROBE_BRIDGE=host:port` set, the app
//! connects out to the strobe performance tool once it can answer, says hello with
//! `STROBE_BRIDGE_TOKEN`, and serves its requests: element queries, waits, input delivered
//! through the same path an OS event takes, window actions, snapshots and frame recording.
//! A reader thread parses requests and wakes the UI thread, which alone touches UI state;
//! a writer thread sends replies. Unset, no thread or socket exists and each hook is one
//! thread-local flag check.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc;

pub(crate) mod chord;
pub(crate) mod clock;
pub(crate) mod driver;
pub(crate) mod headless;
#[cfg(target_os = "macos")]
pub(crate) mod present;
pub(crate) mod protocol;
pub(crate) mod record;
pub(crate) mod select;
pub(crate) mod window;

pub(crate) use driver::Input;
pub(crate) use record::{built, encoded, phase_enter, phase_exit, presented};

#[cfg(test)]
mod tests;

/// A runtime's counters a run differences.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tally {
    pub commits: u64,
    pub shaped: u64,
    pub elements: usize,
}

/// Where to connect and the token to say hello with.
#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    pub addr: String,
    pub token: String,
}

impl Endpoint {
    /// `STROBE_BRIDGE` and `STROBE_BRIDGE_TOKEN`, read once.
    pub(crate) fn from_env() -> Option<Endpoint> {
        static ENDPOINT: std::sync::OnceLock<Option<Endpoint>> = std::sync::OnceLock::new();
        ENDPOINT.get_or_init(|| {
            let var = |name| std::env::var_os(name).and_then(|v| v.into_string().ok());
            let addr = var("STROBE_BRIDGE").filter(|a| !a.is_empty())?;
            Some(Endpoint { addr, token: var("STROBE_BRIDGE_TOKEN").unwrap_or_default() })
        }).clone()
    }
}

/// The connection's writing end: reply lines go out through `out`.
pub(crate) struct Link {
    pub out: mpsc::Sender<String>,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl Link {
    /// Connects, says hello and starts the reader and writer threads; the requests the reader
    /// parses arrive on the receiver. `wake` runs on the reader thread after each request it
    /// queues, and once more when the connection closes.
    pub(crate) fn open(endpoint: &Endpoint, mode: &str, wake: Box<dyn Fn() + Send>) -> Result<(Link, mpsc::Receiver<protocol::Request>), String> {
        let stream = TcpStream::connect(&endpoint.addr).map_err(|e| format!("strobe bridge: connect {}: {e}", endpoint.addr))?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        let mut hello_stream = &stream;
        writeln!(hello_stream, "{}", protocol::hello(&endpoint.token, mode, clock::now_ns())).map_err(|e| e.to_string())?;
        clock::calibrate();
        let (out, lines) = mpsc::channel::<String>();
        let mut write = stream.try_clone().map_err(|e| e.to_string())?;
        let writer = std::thread::Builder::new().name("rune-strobe-writer".into()).spawn(move || {
            // An empty line is `finish`'s: everything before it is written.
            for line in lines {
                if line.is_empty() || writeln!(write, "{line}").is_err() { break; }
            }
            let _ = write.flush();
        }).map_err(|e| e.to_string())?;
        let (tx, requests) = mpsc::channel();
        let errors = out.clone();
        std::thread::Builder::new().name("rune-strobe-reader".into()).spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() { continue; }
                match protocol::parse(&line) {
                    Ok(request) => {
                        if tx.send(request).is_err() { break; }
                        wake();
                    }
                    Err((id, fail)) => { let _ = errors.send(protocol::err(&id, &fail)); }
                }
            }
            // Strobe is gone: dropping `tx` tells the driver, which drops its jobs.
            drop(tx);
            wake();
        }).map_err(|e| e.to_string())?;
        Ok((Link { out, writer: Some(writer) }, requests))
    }

    /// Sends `line` and waits for the writer to flush everything queued.
    pub(crate) fn finish(mut self, line: String) {
        let _ = self.out.send(line);
        let _ = self.out.send(String::new());
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
    }
}

/// What the bridge needs of a window or overlay host, whatever its state type.
pub(crate) trait Surface {
    fn key(&self) -> &str;
    fn overlay(&self) -> bool;
    /// `info`'s entry for it.
    fn describe(&self) -> serde_json::Value;
    fn adapter(&self) -> Option<wgpu::AdapterInfo>;
    fn counts(&self) -> crate::SurfaceCounts;
    fn tally(&self) -> Tally;
    fn find(&self, sel: &select::Selector) -> Option<[f32; 4]>;
    fn query(&self, sel: &select::Selector, all: bool) -> Vec<serde_json::Value>;
    fn quiet(&self) -> bool;
    /// Delivers `input` as the OS would; false when nothing can take it (no window).
    fn deliver(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, input: &Input) -> bool;
    fn window(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, action: &str) -> Result<(), protocol::Fail>;
    /// Renders what it draws now offscreen at its scale: width, height and straight RGBA.
    fn pixels(&mut self) -> Result<(u32, u32, Vec<u8>), protocol::Fail>;
    fn quit(&mut self, event_loop: &winit::event_loop::ActiveEventLoop);
}

/// `info.gpu` for an adapter.
pub(crate) fn gpu_json(info: Option<wgpu::AdapterInfo>) -> serde_json::Value {
    match info {
        Some(a) => serde_json::json!({
            "name": a.name, "backend": format!("{:?}", a.backend), "device_type": format!("{:?}", a.device_type),
            "driver": if a.driver_info.is_empty() { a.driver.clone() } else { format!("{} {}", a.driver, a.driver_info) },
            "vendor": a.vendor,
        }),
        None => serde_json::Value::Null,
    }
}

/// Writes `px` (straight RGBA) to `path` as a PNG.
pub(crate) fn write_snapshot(path: &str, (w, h, px): (u32, u32, Vec<u8>)) -> Result<serde_json::Value, protocol::Fail> {
    crate::render::write_png(std::path::Path::new(path), w, h, &px)
        .map_err(|e| protocol::Fail::new(protocol::Code::Internal, format!("{path}: {e}")))?;
    Ok(serde_json::json!({ "path": path, "width": w, "height": h }))
}
