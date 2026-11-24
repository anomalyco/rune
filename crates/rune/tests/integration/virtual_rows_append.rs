use std::borrow::Cow;
use std::cell::Cell;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, memo, text};

thread_local! {
    static KEY_CALLS: Cell<usize> = const { Cell::new(0) };
}

struct AppendState {
    keys: Vec<String>,
    append_only: bool,
    count_keys: bool,
}

fn append_view(s: &AppendState, _: &mut Cx<AppendState>) -> El {
    let mut list = col().key("rows").w_full().growth(GrowthSpec::default())
        .virtual_rows(
            s.keys.len(),
            24.0,
            |s: &AppendState, i| {
                if s.count_keys {
                    KEY_CALLS.set(KEY_CALLS.get() + 1);
                }
                Cow::Borrowed(s.keys[i].as_str())
            },
            |s: &AppendState, i| memo(s.keys[i].clone(), 0u32, || text(s.keys[i].clone()).h(24.0)),
        )
        .virtual_rows_version(0u32);
    if s.append_only {
        list = list.virtual_rows_append_only();
    }
    col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: true }).child(list)
}

#[test]
fn virtual_rows_append_only_public_api_counts() {
    let measure_calls = |n: usize, append_only: bool| {
        let keys = (0..n).map(|i| format!("req-{i}")).collect();
        let mut rt = Runtime::new(
            AppendState { keys, append_only, count_keys: true },
            append_view,
            (800.0, 600.0),
            Env::default(),
        );
        rt.frame(0.0);
        for _ in 0..8 {
            rt.frame(1.0 / 120.0);
        }
        KEY_CALLS.set(0);
        rt.update(|s, _| {
            let next = s.keys.len();
            s.keys.push(format!("req-{next}"));
        });
        rt.frame(1.0 / 120.0);
        assert!(rt.find(&format!("req-{n}")).is_some());
        KEY_CALLS.get()
    };

    assert_eq!(measure_calls(10_000, false), 10_001);
    let fast_10k = measure_calls(10_000, true);
    let fast_25k = measure_calls(25_000, true);
    assert_eq!(fast_10k, fast_25k, "append_only key calls must stay constant as list grows");
    assert!(fast_10k < 250, "expected O(1) key calls, got {fast_10k}");
    #[cfg(not(debug_assertions))]
    assert_eq!(fast_10k, 4, "release builds check start, midpoint, end and the 1 new key");
}

/// Paired, alternating headless benchmark of appending 1 row per frame to a 100,000-row list
/// (excludes GPU upload and presentation).
/// Run with:
/// `cargo test -p rune --profile perf --test integration virtual_rows_append_time -- --ignored --nocapture`
#[test]
#[ignore]
fn virtual_rows_append_time() {
    fn run_once(append_only: bool) -> f64 {
        const INITIAL: usize = 100_000;
        const FRAMES: usize = 50;
        let keys = (0..INITIAL).map(|i| format!("req-{i}")).collect();
        let mut rt = Runtime::new(
            AppendState { keys, append_only, count_keys: false },
            append_view,
            (800.0, 600.0),
            Env::default(),
        );
        rt.frame(0.0);
        for _ in 0..8 {
            rt.frame(1.0 / 120.0);
        }
        for _ in 0..5 {
            rt.update(|s, _| {
                let next = s.keys.len();
                s.keys.push(format!("req-{next}"));
            });
            rt.frame(1.0 / 120.0);
        }
        let start = rune::time::now();
        for _ in 0..FRAMES {
            rt.update(|s, _| {
                let next = s.keys.len();
                s.keys.push(format!("req-{next}"));
            });
            rt.frame(1.0 / 120.0);
        }
        start.elapsed().as_secs_f64() * 1e3 / FRAMES as f64
    }

    let mut default_ms = Vec::new();
    let mut fast_ms = Vec::new();
    for trial in 0..5 {
        if trial % 2 == 0 {
            default_ms.push(run_once(false));
            fast_ms.push(run_once(true));
        } else {
            fast_ms.push(run_once(true));
            default_ms.push(run_once(false));
        }
    }
    let median = |vals: &[f64]| {
        let mut sorted = vals.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let d_med = median(&default_ms);
    let f_med = median(&fast_ms);
    println!("100k rows, +1 row/frame (headless, excludes GPU upload/present):");
    println!("  default     : median {d_med:.3} ms/frame  runs={default_ms:.3?}");
    println!("  append_only : median {f_med:.3} ms/frame  runs={fast_ms:.3?}");
    println!("  speedup     : {:.1}x", d_med / f_med);
}
