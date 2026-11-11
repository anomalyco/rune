use super::*;
use crate::{Frame, Outcome, Recorder};
use std::sync::{Arc, mpsc::sync_channel};

fn frame(at_ms: u64) -> Sample {
    Sample { at_ms, frame: Frame::new(0, at_ms, Outcome::Drawn).update_ms(300.0).render_ms(1.0) }
}

#[test]
fn writer_publishes_snapshot_and_bounded_incidents_even_when_hook_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = sync_channel(1);
    let config = Config::new(dir.path(), "fixture").build("1.0").incidents(tx);
    let health = Health::default();
    let mut w = Writer::new(&config, Instant::now(), &health);
    for n in 0..40 {
        let mut s = frame(n * 2000);
        s.frame.window = n;
        w.accept(s);
    }
    w.snapshot();
    assert_eq!(w.incidents, 40);
    assert_eq!(w.hook_dropped, 39);
    assert!(rx.try_recv().unwrap().is_file());
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap()).collect();
    assert_eq!(files.iter().filter(|e| e.file_name().to_string_lossy().starts_with("perf-incident-")).count(), 16);
    for e in files.iter().filter(|e| e.path().extension().is_some_and(|s| s == "json")) {
        let bytes = std::fs::read(e.path()).unwrap();
        assert!(bytes.len() <= INCIDENT_BYTES);
        let _: Value = serde_json::from_slice(&bytes).unwrap();
    }
    let snapshot: Value = serde_json::from_slice(&std::fs::read(dir.path().join("perf.json")).unwrap()).unwrap();
    assert_eq!(snapshot["frames"]["total"], 40);
    assert_eq!(snapshot["version"], "1.0");
    assert!(snapshot["frames"]["ui_allocated_bytes"].is_null());
    assert_eq!(snapshot["write_errors"], 0);
}

#[test]
fn logs_rotate_and_failed_writes_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = Log { dir: dir.path(), file: None, size: 0, limit: 128 };
    for n in 0..50 { log.append(&json!({"seq": n})).unwrap(); }
    for name in ["perf.log", "perf.log.1"] {
        let text = std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert!(text.len() <= 128);
        for line in text.lines() { let _: Value = serde_json::from_str(line).unwrap(); }
    }
    // Simulate a prior partial write with stale in-memory size accounting.
    std::fs::write(dir.path().join("perf.log"), vec![b' '; 125]).unwrap();
    log.size = 0;
    log.file = None;
    log.append(&json!({"seq": 51})).unwrap();
    assert!(std::fs::metadata(dir.path().join("perf.log")).unwrap().len() <= 128);
    assert_eq!(std::fs::metadata(dir.path().join("perf.log.1")).unwrap().len(), 125);
    let config = Config::new(dir.path().join("absent"), "fixture");
    let health = Health::default();
    let mut w = Writer::new(&config, Instant::now(), &health);
    w.snapshot();
    assert!(health.errors.load(Relaxed) >= 1);
    assert!(atomic_json(dir.path(), "big.json", &json!("oversize"), 1).is_err());
}

#[test]
fn replacement_wait_is_cancellable_without_touching_owner_files() {
    let dir = tempfile::tempdir().unwrap();
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.path().join(".perf.lock")).unwrap();
    lock.lock().unwrap();
    let recorder = Recorder::start(Config::new(dir.path(), "replacement")).unwrap();
    for _ in 0..1000 { recorder.record(Frame::default()); }
    assert!(recorder.dropped_samples() >= 744);
    recorder.shutdown().unwrap();
    assert!(!dir.path().join("perf.json").exists());
}

#[test]
fn replacement_acquires_after_old_owner_releases() {
    let dir = tempfile::tempdir().unwrap();
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.path().join(".perf.lock")).unwrap();
    lock.lock().unwrap();
    let (tx, rx) = sync_channel(1);
    let config = Config::new(dir.path(), "replacement").incidents(tx);
    let recorder = Recorder::start(config).unwrap();
    recorder.record(frame(0).frame);
    drop(lock);
    let path = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(path.is_file());
    recorder.shutdown().unwrap();
    let snapshot: Value = serde_json::from_slice(&std::fs::read(dir.path().join("perf.json")).unwrap()).unwrap();
    assert_eq!(snapshot["frames"]["total"], 1);
}

#[test]
fn dropped_recorder_cancels_lock_wait() {
    let dir = tempfile::tempdir().unwrap();
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.path().join(".perf.lock")).unwrap();
    lock.lock().unwrap();
    let health = Arc::new(Health::default());
    let h = health.clone();
    let (done, wait) = sync_channel(1);
    let path = dir.path().join(".perf.lock");
    let worker = std::thread::spawn(move || {
        let lock = OpenOptions::new().read(true).write(true).open(path).unwrap();
        done.send(acquire(&lock, &h).unwrap()).unwrap();
    });
    health.stop.store(true, Relaxed);
    assert!(!wait.recv_timeout(Duration::from_secs(5)).unwrap());
    worker.join().unwrap();
}

#[test]
#[ignore = "manual optimized background cost measurement"]
fn instrumentation_overhead_writer() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path(), "benchmark");
    let health = Health::default();
    let mut w = Writer::new(&config, Instant::now(), &health);
    for n in 0..crate::RECENT { w.stats.accept(frame(n as u64)); }
    let mut times = Vec::new();
    for round in 0..22 {
        let at = Instant::now();
        for _ in 0..100 { w.snapshot(); }
        if round > 0 { times.push(at.elapsed().as_secs_f64() * 10.0); }
    }
    println!("METRIC writer_snapshot_ms {}", crate::stats::percentiles(times));
}
