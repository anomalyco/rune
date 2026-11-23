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
fn summary_uses_settled_frame_refresh_rate_over_initial_header() {
    let header = r#"{"type":"header","launch":"rune","width":480,"height":120,"hz":60.0000024000001,"scale":2,"clock":"wall"}"#;
    let trace = [
        header,
        r#"{"t":0,"n":1,"present_ms":16.0}"#,
        r#"{"t":8.3,"n":1,"hz":120,"present_ms":null}"#,
        r#"{"t":16.6,"n":1,"present_ms":24.33,"present_dt_ms":8.33}"#,
        r#"{"t":33.3,"n":1,"present_ms":41.0,"present_dt_ms":16.67}"#,
        r#"{"t":41.6,"n":1,"hz":0,"present_ms":49.33,"present_dt_ms":8.33}"#,
        r#"{"t":66.6,"n":1,"present_ms":74.83,"present_dt_ms":25.5}"#,
    ].join("\n");
    let s = Summary::of_trace(&trace).unwrap();
    assert_eq!((s.hz, s.frames, s.presented, s.skipped, s.missed), (120.0, 6, 5, 1, 2));
    assert!(s.to_string().contains("at 120 Hz") && s.to_string().contains("2 missed (>12.50 ms)"), "{s}");
}

#[test]
fn spread_uses_nearest_rank() {
    let s = Spread::of((1..=20).map(f64::from).collect()).unwrap();
    assert_eq!((s.p50, s.p95, s.max), (10.0, 19.0, 20.0));
    assert_eq!(Spread::of(vec![4.0]), Some(Spread { p50: 4.0, p95: 4.0, max: 4.0 }));
    assert_eq!(Spread::of(Vec::new()), None);
}

#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn kvo_observed_layer_hook_in_both_orders() {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSObject, NSString};

    let key = NSString::from_str("probe");
    let observer = NSObject::new();

    // Order 1: LayerHook first, then KVO observes, hook drops, observer comes off, re-hook reuses class.
    // SAFETY: live `NSObject` instances; KVO selectors match Foundation.
    unsafe {
        let below: Retained<AnyObject> = msg_send![class!(NSObject), new];
        let log = Arc::new(metal::Log::untraced());
        let hook = metal::LayerHook::hook_instance(below.clone(), log.clone()).expect("hook plain object");
        let hooked_class = below.class();
        assert!(hooked_class.name().to_bytes().starts_with(b"RunePresentTimed_"));

        let _: () = msg_send![&*below, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
        let kvo_class = below.class();
        assert!(kvo_class.name().to_bytes().starts_with(b"NSKVONotifying_"));

        drop(hook);
        assert!(std::ptr::eq(below.class(), kvo_class), "dropping hook while KVO is on top leaves KVO intact");
        let _: () = msg_send![&*below, removeObserver: &*observer, forKeyPath: &*key];

        // After KVO restores our leftover class, re-hooking reuses it rather than stacking a second one.
        let again = metal::LayerHook::hook_instance(below.clone(), log).expect("re-hook leftover class");
        assert!(std::ptr::eq(below.class(), hooked_class));
        drop(again);
        assert!(!below.class().name().to_bytes().starts_with(b"RunePresentTimed_"));
    }

    // Order 2: KVO observes first, then LayerHook skips it, then observer comes off.
    // SAFETY: live `NSObject` instances; KVO selectors match Foundation.
    unsafe {
        let above: Retained<AnyObject> = msg_send![class!(NSObject), new];
        let _: () = msg_send![&*above, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
        let kvo_class = above.class();
        assert!(kvo_class.name().to_bytes().starts_with(b"NSKVONotifying_"));

        let log = Arc::new(metal::Log::untraced());
        assert!(metal::LayerHook::hook_instance(above.clone(), log).is_none(), "skip already KVO-observed object");
        assert!(std::ptr::eq(above.class(), kvo_class), "KVO's class must stay on top");
        let _: () = msg_send![&*above, removeObserver: &*observer, forKeyPath: &*key];
    }
}
