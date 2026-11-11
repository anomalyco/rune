use std::{fs::{File, OpenOptions, TryLockError}, io::{self, Write}, path::Path,
    sync::{atomic::Ordering::Relaxed, mpsc::{Receiver, RecvTimeoutError}}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use serde_json::{Value, json};
use procwatch::Usage;
use crate::{Config, LARGE_BYTES, SLOW_MS, Sample, incident::{Detector, Finding}, recorder::Health, stats::Stats};

const LOG_BYTES: u64 = 4 << 20;
const INCIDENT_BYTES: usize = 256 << 10;
/// How often, at most, process memory is checked for incidents: as frames arrive, never on a
/// timer of its own, so an idle app wakes nothing.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// What was found is kept in memory and written once the app has been this long without
/// another slow frame or incident, so a burst (typing in a slow build) is one write, after it.
const QUIET: Duration = Duration::from_secs(2);
/// Memory lines kept while nothing else is to be written; they go out only beside a finding.
const MEMORY_LINES: usize = 20;

pub(crate) fn run(config: Config, rx: Receiver<Sample>, start: Instant, health: &Health) -> io::Result<()> {
    std::fs::create_dir_all(&config.directory)?;
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(config.directory.join(".perf.lock"))?;
    if !acquire(&lock, health)? { return Ok(()); }
    let mut writer = Writer::new(&config, start, health);
    let mut checked: Option<Instant> = None;
    loop {
        let got = match writer.due {
            Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(s) => {
                writer.accept(s);
                if checked.is_none_or(|at| at.elapsed() >= CHECK_EVERY) {
                    writer.check();
                    checked = Some(Instant::now());
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                writer.check();
                writer.flush();
                if writer.wrote { writer.snapshot(); }
                break;
            }
            Err(RecvTimeoutError::Timeout) => writer.flush(),
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
    /// Log lines and incident files found since the last flush, and when to write them.
    lines: Vec<Value>,
    found: Vec<(String, Value)>,
    due: Option<Instant>,
    /// Anything was written this run, so the shutdown snapshot goes out too.
    wrote: bool,
}

impl<'a> Writer<'a> {
    fn new(config: &'a Config, start: Instant, health: &'a Health) -> Self {
        let started_at = now_ms().saturating_sub(start.elapsed().as_millis() as u64);
        Self { config, health, start, launch: format!("{}-{started_at}", std::process::id()), started_at,
            stats: Stats::default(), detector: Detector::default(), memory_at: 0, incidents: 0, hook_dropped: 0,
            log: Log { dir: &config.directory, file: None, size: 0, limit: LOG_BYTES },
            lines: Vec::new(), found: Vec::new(), due: None, wrote: false }
    }

    fn accept(&mut self, s: Sample) {
        if self.stats.accept(s) {
            self.lines.push(json!({"kind": "frame", "launch": self.launch, "frame": s.json()}));
            self.due = Some(Instant::now() + QUIET);
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
        self.incidents += 1;
        // A later incident in the same slot replaces this one before it's written: its
        // notification is lost, as a full receiver's is.
        let before = self.found.len();
        self.found.retain(|(n, _)| *n != name);
        if self.found.len() < before && self.config.incidents.is_some() { self.hook_dropped += 1; }
        self.found.push((name, value));
        self.due = Some(Instant::now() + QUIET);
    }

    /// Writes what was found since the last flush: the log lines, then each incident file and a
    /// snapshot beside them. Nothing when nothing was found; memory lines alone wait.
    fn flush(&mut self) {
        self.due = None;
        let findings = self.lines.iter().any(|l| l["kind"] != "memory") || !self.found.is_empty();
        if !findings { return; }
        self.wrote = true;
        for line in std::mem::take(&mut self.lines) {
            let result = self.log.append(&line);
            self.error(result);
        }
        let found = std::mem::take(&mut self.found);
        let any = !found.is_empty();
        for (name, value) in found {
            match atomic_json(&self.config.directory, &name, &value, INCIDENT_BYTES) {
                Ok(()) => {
                    if let Some(hook) = &self.config.incidents
                        && hook.try_send(self.config.directory.join(name)).is_err() { self.hook_dropped += 1; }
                }
                Err(e) => self.error(Err(e)),
            }
        }
        if any { self.write_snapshot(); }
    }

    /// Writes what is still in memory, then `perf.json`.
    fn snapshot(&mut self) {
        self.flush();
        self.write_snapshot();
    }

    /// The periodic check: process memory against the incident rules, and a memory line in the
    /// log every 30 s. No snapshot.
    fn check(&mut self) {
        let now = self.start.elapsed().as_millis() as u64;
        self.stats.prune(now);
        let memory = procwatch::usage();
        for f in self.detector.process(now, memory) { self.incident(now, f); }
        if now >= self.memory_at {
            let memory_lines = self.lines.iter().filter(|l| l["kind"] == "memory").count();
            if memory_lines >= MEMORY_LINES && let Some(i) = self.lines.iter().position(|l| l["kind"] == "memory") { self.lines.remove(i); }
            self.lines.push(json!({"kind": "memory", "at_ms": now, "launch": self.launch, "memory": usage_json(memory)}));
            self.memory_at = now + 30_000;
        }
    }

    /// `perf.json`: beside each flush with incidents, and at shutdown when this run wrote anything.
    fn write_snapshot(&mut self) {
        let now = self.start.elapsed().as_millis() as u64;
        let memory = procwatch::usage();
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

/// `perf.log`, kept open for appending: a line costs one write, not an open, a stat and a close.
struct Log<'a> { dir: &'a Path, file: Option<File>, size: u64, limit: u64 }
impl Log<'_> {
    fn append(&mut self, value: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        if line.len() as u64 > self.limit { return Err(io::Error::other("performance record exceeds log limit")); }
        let path = self.dir.join("perf.log");
        if self.file.is_none() {
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            self.size = file.metadata()?.len();
            self.file = Some(file);
        }
        if self.size + line.len() as u64 > self.limit {
            self.file = None;
            std::fs::rename(&path, self.dir.join("perf.log.1"))?;
            self.file = Some(OpenOptions::new().create(true).append(true).open(&path)?);
            self.size = 0;
        }
        let Some(file) = self.file.as_mut() else { return Ok(()) };
        if let Err(e) = file.write_all(&line) {
            // A partial write (a full disk): the next line reopens and reads the real size.
            self.file = None;
            return Err(e);
        }
        self.size += line.len() as u64;
        Ok(())
    }
}
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }

#[cfg(test)]
mod tests;
