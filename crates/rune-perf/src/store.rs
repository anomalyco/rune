use std::{fs::{File, OpenOptions, TryLockError}, io::{self, Write}, path::Path,
    sync::{atomic::Ordering::Relaxed, mpsc::{Receiver, RecvTimeoutError}}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use serde_json::{Value, json};
use procwatch::Usage;
use crate::{Config, LARGE_BYTES, SLOW_MS, Sample, incident::{Detector, Finding}, recorder::Health, stats::Stats};

const LOG_BYTES: u64 = 4 << 20;
const INCIDENT_BYTES: usize = 256 << 10;
const SNAPSHOT_EVERY: Duration = Duration::from_secs(2);

pub(crate) fn run(config: Config, rx: Receiver<Sample>, start: Instant, health: &Health) -> io::Result<()> {
    std::fs::create_dir_all(&config.directory)?;
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(config.directory.join(".perf.lock"))?;
    if !acquire(&lock, health)? { return Ok(()); }
    let mut writer = Writer::new(&config, start, health);
    let mut next = Instant::now();
    loop {
        match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(s) => writer.accept(s),
            Err(RecvTimeoutError::Disconnected) => { writer.snapshot(); break; }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= next {
            writer.snapshot();
            next = Instant::now() + SNAPSHOT_EVERY;
        }
    }
    Ok(())
}

fn acquire(lock: &File, health: &Health) -> io::Result<bool> {
    loop {
        if health.stop.load(Relaxed) { return Ok(false); }
        match lock.try_lock() {
            Ok(()) => return Ok(true),
            Err(TryLockError::WouldBlock) => std::thread::park_timeout(Duration::from_millis(100)),
            Err(TryLockError::Error(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }
}

struct Writer<'a> {
    config: &'a Config,
    health: &'a Health,
    start: Instant,
    launch: String,
    started_at: u64,
    stats: Stats,
    detector: Detector,
    log: Log<'a>,
    memory_at: u64,
    incidents: u64,
    hook_dropped: u64,
}

impl<'a> Writer<'a> {
    fn new(config: &'a Config, start: Instant, health: &'a Health) -> Self {
        let started_at = now_ms().saturating_sub(start.elapsed().as_millis() as u64);
        Self { config, health, start, launch: format!("{}-{started_at}", std::process::id()), started_at,
            stats: Stats::default(), detector: Detector::default(), memory_at: 0, incidents: 0, hook_dropped: 0,
            log: Log { dir: &config.directory, size: std::fs::metadata(config.directory.join("perf.log")).map_or(0, |m| m.len()), limit: LOG_BYTES } }
    }

    fn accept(&mut self, s: Sample) {
        if self.stats.accept(s) {
            let result = self.log.append(&json!({"kind": "frame", "launch": self.launch, "frame": s.json()}));
            self.error(result);
        }
        if let Some(f) = self.detector.frame(s, &self.stats) { self.incident(s.at_ms, f); }
    }

    fn incident(&mut self, now: u64, f: Finding) {
        if !self.detector.admit(now, &f) { return; }
        let value = json!({"schema": 1, "app": self.config.app, "build": self.config.build,
            "launch": self.launch, "pid": std::process::id(), "created_at_ms": now_ms(),
            "rule": f.rule.id(), "window": f.window, "source": f.source, "evidence": f.evidence,
            "history": self.stats.recent.iter().map(|s| s.json()).collect::<Vec<_>>(),
            "dropped_samples": self.health.dropped.load(Relaxed)});
        let name = format!("perf-incident-{:02}.json", self.incidents % 16);
        match atomic_json(&self.config.directory, &name, &value, INCIDENT_BYTES) {
            Ok(()) => {
                self.incidents += 1;
                if let Some(hook) = &self.config.incidents
                    && hook.try_send(self.config.directory.join(name)).is_err() { self.hook_dropped += 1; }
            }
            Err(e) => self.error(Err(e)),
        }
    }

    fn snapshot(&mut self) {
        let now = self.start.elapsed().as_millis() as u64;
        self.stats.prune(now);
        let memory = procwatch::usage();
        for f in self.detector.process(now, memory) { self.incident(now, f); }
        let frames = self.stats.json();
        let snapshot = json!({"schema": 1, "recorder_version": env!("CARGO_PKG_VERSION"),
            "app": self.config.app, "version": self.config.build, "launch": self.launch,
            "pid": std::process::id(), "started_at_ms": self.started_at, "updated_at_ms": now_ms(), "uptime_ms": now,
            "scope": "work_ms = update + compositor wall time; input_to_frame_ms ends at encoding, not presentation; allocation requests are not retained heap",
            "slow_ms": SLOW_MS, "large_bytes": LARGE_BYTES, "memory": usage_json(memory),
            "capabilities": {"process_memory": memory.resident.is_some(), "file_descriptors": memory.fds.is_some(),
                "ui_allocations": !frames["ui_allocated_bytes"].is_null(),
                "input_to_frame": !frames["input_to_frame_ms"].is_null(), "gpu_timestamps": !frames["gpu_ms"].is_null(),
                "presentation": false},
            "dropped_samples": self.health.dropped.load(Relaxed), "write_errors": self.health.errors.load(Relaxed),
            "incidents": self.incidents, "incidents_suppressed": self.detector.suppressed,
            "incident_notifications_dropped": self.hook_dropped, "frames": frames});
        self.error(atomic_json(&self.config.directory, "perf.json", &snapshot, INCIDENT_BYTES));
        if now >= self.memory_at {
            let result = self.log.append(&json!({"kind": "memory", "at_ms": now, "launch": self.launch, "memory": usage_json(memory)}));
            self.error(result);
            self.memory_at = now + 30_000;
        }
    }

    fn error(&self, result: io::Result<()>) {
        if let Err(e) = result
            && self.health.errors.fetch_add(1, Relaxed) == 0 { eprintln!("rune-perf: write failed: {e}"); }
    }
}

fn usage_json(u: Usage) -> Value {
    json!({"resident_bytes": u.resident, "footprint_bytes": u.footprint,
        "peak_footprint_bytes": u.footprint_peak, "open_fds": u.fds})
}

fn atomic_json(dir: &Path, name: &str, value: &Value, limit: usize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > limit { return Err(io::Error::other("performance record exceeds byte cap")); }
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, dir.join(name))
}

struct Log<'a> { dir: &'a Path, size: u64, limit: u64 }
impl Log<'_> {
    fn append(&mut self, value: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        if line.len() as u64 > self.limit { return Err(io::Error::other("performance record exceeds log limit")); }
        let path = self.dir.join("perf.log");
        // A previous write_all may have failed after a partial write (for example disk full).
        // Re-read the actual size so retries cannot exceed the cap with stale accounting.
        self.size = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        if self.size + line.len() as u64 > self.limit {
            std::fs::rename(&path, self.dir.join("perf.log.1"))?;
            self.size = 0;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(&line)?;
        self.size = file.metadata()?.len();
        Ok(())
    }
}

#[expect(clippy::disallowed_methods, reason = "wall-clock timestamp in perf log header")]
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }

#[cfg(test)]
mod tests;
