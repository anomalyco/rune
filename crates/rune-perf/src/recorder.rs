use std::{io, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering::Relaxed}, mpsc::{SyncSender, sync_channel}}, thread::JoinHandle, time::Instant};
use crate::{Config, Frame, QUEUE, Sample, store};

#[derive(Default)]
pub(crate) struct Health {
    pub dropped: AtomicU64,
    pub errors: AtomicU64,
    pub stop: AtomicBool,
}

/// Owns one writer. Drop cancels without blocking the UI; `shutdown` drains and joins off-UI.
/// Dropping while waiting for a previous process's directory lock cancels that wait too.
pub struct Recorder {
    start: Instant,
    out: Option<SyncSender<Sample>>,
    health: Arc<Health>,
    worker: Option<JoinHandle<()>>,
}

impl Recorder {
    #[expect(clippy::disallowed_methods, reason = "dedicated off-UI disk writer thread")]
    pub fn start(config: Config) -> io::Result<Self> {
        if config.app.len() > 128 || config.build.len() > 256 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "app/build identity too long"));
        }
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

    /// Nonblocking, fixed-size send. Returns false when the queue is full or writer stopped.
    pub fn record(&self, frame: Frame) -> bool {
        let sample = Sample { at_ms: self.start.elapsed().as_millis() as u64, frame: frame.sanitize() };
        if self.out.as_ref().is_some_and(|out| out.try_send(sample).is_ok()) { return true; }
        self.health.dropped.fetch_add(1, Relaxed);
        false
    }

    pub fn dropped_samples(&self) -> u64 { self.health.dropped.load(Relaxed) }
    pub fn write_errors(&self) -> u64 { self.health.errors.load(Relaxed) }

    /// Call off the UI thread. Cancels lock waits; drains queued frames if already recording.
    pub fn shutdown(mut self) -> std::thread::Result<()> {
        self.health.stop.store(true, Relaxed);
        self.out.take();
        self.worker.take().map_or(Ok(()), |w| w.join())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) { self.health.stop.store(true, Relaxed); }
}

#[cfg(test)]
mod tests;
