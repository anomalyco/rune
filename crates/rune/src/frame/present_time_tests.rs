use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Fake(Mutex<Vec<Presented>>);
impl Source for Fake {
    fn take(&self, out: &mut Vec<Presented>) { out.append(&mut self.0.lock().unwrap()); }
}

fn written(out: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(out).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn lines_wait_for_their_present_time_and_keep_their_order() {
    let start = crate::time::now();
    let ms = |n: u64| start + Duration::from_micros(n);
    let fake = Arc::new(Fake::default());
    let mut presents = Presents::new(fake.clone(), start);
    let mut out = Vec::new();
    presents.push(r#"{"t":1"#.into(), Some(1), false, &mut out);
    presents.push(r#"{"t":2"#.into(), Some(2), true, &mut out);
    assert!(out.is_empty(), "no line before its present time");
    // Frame 2's present arrives first; frame 1 still holds it back.
    fake.0.lock().unwrap().push((2, Some(ms(33_333))));
    presents.push(r#"{"t":3"#.into(), None, true, &mut out);
    assert!(out.is_empty() && presents.waiting());
    fake.0.lock().unwrap().extend([(1, Some(ms(25_000)))]);
    presents.push(r#"{"t":4"#.into(), Some(3), true, &mut out);
    let lines = written(&out);
    assert_eq!(lines.len(), 3, "frames 1–3 complete in order: {lines:?}");
    assert_eq!(lines[0]["present_ms"], 25.0);
    assert!(lines[0].get("present_dt_ms").is_none(), "the first present has no interval");
    assert_eq!(lines[1]["present_ms"], 33.333);
    assert_eq!(lines[1]["present_dt_ms"], 8.333);
    assert!(lines[2].get("present_ms").is_none(), "a frame without a drawable has no present field");
    fake.0.lock().unwrap().push((3, None));
    presents.flush(&mut out);
    let lines = written(&out);
    assert_eq!(lines[3]["t"], 4);
    assert!(lines[3]["present_ms"].is_null(), "a skipped drawable is null");
}

#[test]
fn idle_gaps_and_lost_presents_do_not_count_as_intervals() {
    let start = crate::time::now();
    let fake = Arc::new(Fake::default());
    let mut presents = Presents::new(fake.clone(), start);
    let mut out = Vec::new();
    presents.push(r#"{"t":0"#.into(), Some(1), false, &mut out);
    fake.0.lock().unwrap().push((1, Some(start)));
    // After idle: presented 500 ms later, no interval.
    presents.push(r#"{"t":500"#.into(), Some(2), false, &mut out);
    fake.0.lock().unwrap().push((2, Some(start + Duration::from_millis(500))));
    // Never reported: written without a present after WAIT_FRAMES later frames.
    presents.push(r#"{"t":508"#.into(), Some(3), true, &mut out);
    for k in 0..WAIT_FRAMES { presents.push(format!(r#"{{"t":{}"#, 600 + k), None, true, &mut out); }
    let lines = written(&out);
    assert_eq!(lines.len(), 3 + WAIT_FRAMES as usize);
    assert!(lines[1].get("present_dt_ms").is_none(), "an idle gap is not cadence");
    assert!(lines[2].get("present_ms").is_none(), "a present that never came is left out, not faked");
}

/// Frames `(seq, continuous, present ms)` around a gap; `None` present: never reported.
/// Returns the last line, which presented 508 ms in, right after the frame before it.
fn after_gap(frames: &[(Option<u64>, bool, Option<Option<u64>>)]) -> serde_json::Value {
    let start = crate::time::now();
    let fake = Arc::new(Fake::default());
    let mut presents = Presents::new(fake.clone(), start);
    let mut out = Vec::new();
    presents.push(r#"{"t":0"#.into(), Some(100), true, &mut out);
    fake.0.lock().unwrap().push((100, Some(start)));
    for (k, &(seq, continuous, present)) in frames.iter().enumerate() {
        presents.push(format!(r#"{{"t":{}"#, k + 1), seq, continuous, &mut out);
        if let (Some(seq), Some(at)) = (seq, present) {
            fake.0.lock().unwrap().push((seq, at.map(|ms| start + Duration::from_millis(ms))));
        }
    }
    presents.push(r#"{"t":"last""#.into(), Some(200), true, &mut out);
    fake.0.lock().unwrap().push((200, Some(start + Duration::from_millis(508))));
    presents.flush(&mut out);
    let last = written(&out).pop().unwrap();
    assert_eq!((last["t"].as_str(), last["present_ms"].as_f64()), (Some("last"), Some(508.0)));
    last
}

#[test]
fn a_skipped_first_frame_after_idle_starts_no_interval() {
    let last = after_gap(&[(Some(1), false, Some(None))]);
    assert!(last.get("present_dt_ms").is_none(), "the idle gap is not cadence: {last}");
}

#[test]
fn a_frame_without_a_drawable_breaks_the_interval() {
    let last = after_gap(&[(None, true, None)]);
    assert!(last.get("present_dt_ms").is_none(), "{last}");
}

#[test]
fn a_present_that_never_came_breaks_the_interval() {
    let last = after_gap(&[(Some(1), true, None)]);
    assert!(last.get("present_dt_ms").is_none(), "{last}");
    // The interval still spans two presented frames drawn back to back.
    assert_eq!(after_gap(&[(Some(1), true, Some(Some(500)))])["present_dt_ms"], 8.0);
}

const HEADER: &str = r#"{"type":"header","launch":"rune","width":480,"height":120,"hz":120.0,"scale":2,"clock":"wall"}"#;

#[test]
fn summary_counts_bunched_and_missed_intervals_and_lag() {
    let mut trace = vec![HEADER.to_owned()];
    // target, present, interval
    let frames: [(f64, Option<f64>, Option<f64>); 7] = [
        (8.0, Some(25.0), None),
        (16.4, Some(33.3), Some(8.3)),
        (24.7, Some(33.8), Some(0.5)), // bunched
        (33.0, Some(34.6), Some(0.8)), // bunched
        (41.4, Some(58.3), Some(23.7)), // missed (> 12.5 ms)
        (49.7, None, None), // skipped
        (58.0, Some(66.7), Some(8.4)),
    ];
    for (k, (target, present, dt)) in frames.into_iter().enumerate() {
        let mut line = serde_json::json!({ "t": k, "n": 1, "target_ms": target });
        line["present_ms"] = present.into();
        if let Some(dt) = dt { line["present_dt_ms"] = dt.into(); }
        trace.push(line.to_string());
    }
    let s = Summary::of_trace(&trace.join("\n")).unwrap();
    assert_eq!((s.frames, s.presented, s.skipped, s.bunched, s.missed), (7, 6, 1, 2, 1));
    let i = s.interval.unwrap();
    assert_eq!((i.p50, i.p95, i.max), (8.3, 23.7, 23.7));
    let lag = s.lag.unwrap();
    assert!((lag.p50 - 9.1).abs() < 1e-9 && (lag.max - 17.0).abs() < 1e-9, "{lag:?}");
    assert!(s.to_string().contains("2 bunched (<2 ms), 1 missed (>12.50 ms)"), "{s}");
}

#[test]
fn traces_without_present_times_have_no_summary() {
    let old = [HEADER, r#"{"t":0,"n":1,"quality":{"cpu_ms":0.2}}"#, r#"{"t":8.33,"n":1,"e":[[1,0,0,10,10,1,0,0]]}"#].join("\n");
    assert_eq!(Summary::of_trace(&old), None);
    // A trace with targets but no present source (another platform) has none either.
    let targets = [HEADER, r#"{"t":0,"n":1,"target_ms":8.0}"#].join("\n");
    assert_eq!(Summary::of_trace(&targets), None);
    assert_eq!(Summary::of_trace(""), None);
}

#[test]
fn spread_uses_nearest_rank() {
    let s = Spread::of((1..=20).map(f64::from).collect()).unwrap();
    assert_eq!((s.p50, s.p95, s.max), (10.0, 19.0, 20.0));
    assert_eq!(Spread::of(vec![4.0]), Some(Spread { p50: 4.0, p95: 4.0, max: 4.0 }));
    assert_eq!(Spread::of(Vec::new()), None);
}
