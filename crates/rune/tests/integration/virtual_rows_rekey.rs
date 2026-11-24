use std::borrow::Cow;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, memo, text};

struct RekeyState {
    keys: Vec<String>,
    version: u64,
    row_height: f32,
}

fn rekey_view(s: &RekeyState, _: &mut Cx<RekeyState>) -> El {
    col().key("scroll").w_full().h_full().scroll(ScrollSpec::default()).child(
        col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(
                s.keys.len(),
                20.0,
                |s: &RekeyState, i| Cow::Borrowed(s.keys[i].as_str()),
                |s: &RekeyState, i| memo(s.keys[i].clone(), 0u32, || el().w_full().h(s.row_height)),
            )
            .virtual_rows_version(s.version),
    )
}

#[test]
fn virtual_rows_rekey_preserves_anchor_on_prepend_and_clears_on_disjoint_replace() {
    let keys = (0..2_000).map(|i| format!("r{i}")).collect();
    let mut rt = Runtime::new(
        RekeyState { keys, version: 0, row_height: 36.0 },
        rekey_view,
        (400.0, 400.0),
        Env::default(),
    );
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 10_000.0));
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }

    let before = rt.elements().into_iter()
        .find(|e| e.path.starts_with("scroll/rows/r") && e.visible[3] > 0.0)
        .expect("visible row");
    let before_top = before.rect[1];

    // Prepend 50 rows with a version bump: the visible anchor must stay at the same viewport y.
    rt.update(|s, _| {
        let mut next: Vec<String> = (0..50).map(|i| format!("pre-{i}")).collect();
        next.extend(s.keys.iter().cloned());
        s.keys = next;
        s.version += 1;
    });
    rt.frame(1.0 / 60.0);
    let after = rt.elements().into_iter()
        .find(|e| e.path == before.path)
        .expect("anchor row stays mounted after prepend");
    assert!(
        (after.rect[1] - before_top).abs() < 0.5,
        "prepend shifted anchor row from {before_top} to {}",
        after.rect[1],
    );

    // Replace all keys (simulating a column-count change): mounts new rows cleanly.
    rt.update(|s, _| {
        s.keys = (0..1_500).map(|i| format!("col6-{i}")).collect();
        s.version += 1;
    });
    rt.frame(1.0 / 60.0);
    assert!(
        rt.elements().iter().any(|e| e.path.starts_with("scroll/rows/col6-")),
        "disjoint rekey must mount the new rows",
    );
}

#[test]
fn virtual_rows_rekey_renames_duplicate_keys_without_panicking() {
    struct S { keys: Vec<String>, version: u32 }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("scroll").w_full().h_full().scroll(ScrollSpec::default()).child(
            col().key("rows").w_full().growth(GrowthSpec::default())
                .virtual_rows(
                    s.keys.len(),
                    20.0,
                    |s: &S, i| Cow::Borrowed(s.keys[i].as_str()),
                    |s: &S, i| text(s.keys[i].clone()).h(20.0),
                )
                .virtual_rows_version(s.version),
        )
    }
    let mut rt = Runtime::new(
        S { keys: vec!["x".into(), "x".into(), "x#dup1".into()], version: 0 },
        view,
        (300.0, 200.0),
        Env::default(),
    );
    rt.frame(0.0);
    assert!(rt.find("x").is_some());
    assert!(rt.find("x#dup2").is_some());
    assert!(rt.find("x#dup1").is_some());

    rt.update(|s, _| {
        s.keys = vec!["y".into(), "y".into(), "y".into()];
        s.version = 1;
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("y").is_some());
    assert!(rt.find("y#dup1").is_some());
    assert!(rt.find("y#dup2").is_some());
}

/// Headless benchmark of rekeying a 166,668-row `virtual_rows` list (excludes GPU upload and
/// presentation): initial load, disjoint version-change rekey (column-count resize), overlapping
/// count+version change, and same-count reorder.
/// Run with:
/// `cargo test -p rune --profile perf --test integration virtual_rows_rekey_166k_time -- --ignored --nocapture`
#[test]
#[ignore]
fn virtual_rows_rekey_166k_time() {
    const N: usize = 166_668;
    const TRIALS: usize = 7;

    let keys_a: Vec<String> = (0..N).map(|i| format!("g8-{i}")).collect();
    let keys_b: Vec<String> = (0..N).map(|i| format!("g6-{i}")).collect();
    let mut keys_prepend = Vec::with_capacity(N + 1);
    keys_prepend.push("g6-head".to_owned());
    keys_prepend.extend(keys_b.iter().cloned());
    let mut keys_reorder = keys_prepend.clone();
    keys_reorder.swap(0, 1);

    let mut init_ms = Vec::with_capacity(TRIALS);
    let mut disjoint_ms = Vec::with_capacity(TRIALS);
    let mut overlap_ms = Vec::with_capacity(TRIALS);
    let mut reorder_ms = Vec::with_capacity(TRIALS);

    // Warm up once before timing.
    {
        let mut rt = Runtime::new(
            RekeyState { keys: keys_a.clone(), version: 0, row_height: 24.0 },
            rekey_view,
            (800.0, 600.0),
            Env::default(),
        );
        rt.frame(0.0);
    }

    for _ in 0..TRIALS {
        let mut rt = Runtime::new(
            RekeyState { keys: keys_a.clone(), version: 0, row_height: 24.0 },
            rekey_view,
            (800.0, 600.0),
            Env::default(),
        );
        let t0 = rune::time::now();
        rt.frame(0.0);
        init_ms.push(t0.elapsed().as_secs_f64() * 1e3);

        rt.update(|s, _| {
            s.keys = keys_b.clone();
            s.version = 1;
        });
        let t1 = rune::time::now();
        rt.frame(1.0 / 120.0);
        disjoint_ms.push(t1.elapsed().as_secs_f64() * 1e3);

        rt.update(|s, _| {
            s.keys = keys_prepend.clone();
            s.version = 2;
        });
        let t2 = rune::time::now();
        rt.frame(1.0 / 120.0);
        overlap_ms.push(t2.elapsed().as_secs_f64() * 1e3);

        rt.update(|s, _| {
            s.keys = keys_reorder.clone();
            s.version = 3;
        });
        let t3 = rune::time::now();
        rt.frame(1.0 / 120.0);
        reorder_ms.push(t3.elapsed().as_secs_f64() * 1e3);
    }

    let median = |vals: &[f64]| {
        let mut s = vals.to_vec();
        s.sort_by(f64::total_cmp);
        s[s.len() / 2]
    };
    println!("166,668 virtual_rows rekey (headless, excludes GPU upload/present):");
    println!("  initial_load  : median {:.3} ms  runs={init_ms:.3?}", median(&init_ms));
    println!("  disjoint_rekey: median {:.3} ms  runs={disjoint_ms:.3?}", median(&disjoint_ms));
    println!("  overlap_rekey : median {:.3} ms  runs={overlap_ms:.3?}", median(&overlap_ms));
    println!("  reorder_rekey : median {:.3} ms  runs={reorder_ms:.3?}", median(&reorder_ms));
}
