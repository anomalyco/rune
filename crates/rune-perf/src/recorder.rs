use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::{sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering::Relaxed}, mpsc::{SyncSender, sync_channel}}, thread::JoinHandle, time::Instant};
#[cfg(not(target_arch = "wasm32"))]
use crate::{QUEUE, Sample, store};
use crate::{Config, Frame};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct Health {
    pub dropped: AtomicU64,
    pub errors: AtomicU64,
    pub stop: AtomicBool,
}

/// Owns one writer. Drop cancels without blocking the UI; `shutdown` drains and joins off-UI.
/// Dropping while waiting for a previous process's directory lock cancels that wait too.
pub struct Recorder {
    #[cfg(not(target_arch = "wasm32"))]
    start: Instant,
    #[cfg(not(target_arch = "wasm32"))]
    out: Option<SyncSender<Sample>>,
    #[cfg(not(target_arch = "wasm32"))]
    health: Arc<Health>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<JoinHandle<()>>,
}

impl Recorder {
    pub fn start(config: Config) -> io::Result<Self> {
        if config.app.len() > 128 || config.build.len() > 256 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "app/build identity too long"));
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(io::Error::new(io::ErrorKind::Unsupported, "local perf files are unsupported on web"))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (out, rx) = sync_channel(QUEUE);
            let start = Instant::now();
            let health = Arc::new(Health::default());
            let h = health.clone();
            // All filesystem work, including mkdir/open/locking, belongs to this thread.
            let worker = std::thread::Builder::new().name("rune-perf".into()).spawn(move || {
                if let Err(e) = store::run(config, rx, start, &h) {
                    h.errors.fetch_add(1, Relaxed);
                    eprintln!("rune-perf: recorder stopped: {e}");
                }
            })?;
            Ok(Self { start, out: Some(out), health, worker: Some(worker) })
        }
    }

    /// Nonblocking, fixed-size send. Returns false when the queue is full or writer stopped.
    pub fn record(&self, frame: Frame) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = frame;
            false
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let sample = Sample { at_ms: self.start.elapsed().as_millis() as u64, frame: frame.sanitize() };
            if self.out.as_ref().is_some_and(|out| out.try_send(sample).is_ok()) { return true; }
            self.health.dropped.fetch_add(1, Relaxed);
            false
        }
    }

    pub fn dropped_samples(&self) -> u64 {
        #[cfg(target_arch = "wasm32")]
        {
            0
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.health.dropped.load(Relaxed)
        }
    }

    /// Call off the UI thread. Cancels lock waits; drains queued frames if already recording.
    pub fn shutdown(mut self) -> std::thread::Result<()> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &mut self;
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.health.stop.store(true, Relaxed);
            self.out.take();
            self.worker.take().map_or(Ok(()), |w| w.join())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Recorder {
    fn drop(&mut self) { self.health.stop.store(true, Relaxed); }
}

#[cfg(test)]
mod tests;
