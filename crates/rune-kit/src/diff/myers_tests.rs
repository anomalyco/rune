use super::*;

fn apply<T: Clone + Eq + std::fmt::Debug>(a: &[T], b: &[T], ops: &[Op]) -> (Vec<T>, usize) {
    let mut out = Vec::new();
    let mut same = 0;
    for op in ops {
        match *op {
            Op::Equal { old, new, len } => {
                assert_eq!(a[old..old + len], b[new..new + len]);
                out.extend_from_slice(&a[old..old + len]);
                same += len;
            }
            Op::Delete { .. } => {}
            Op::Insert { new, len } => out.extend_from_slice(&b[new..new + len]),
        }
    }
    (out, same)
}

fn lcs<T: Eq>(a: &[T], b: &[T]) -> usize {
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    dp[0][0]
}

#[test]
fn scripts_are_minimal_and_rebuild_the_new_side() {
    let mut seed = 0x2545_f491_u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for round in 0..1200 {
        let (len, alphabet) = if round % 3 == 0 { (40, 3) } else { (14, 4) };
        let (la, lb) = (next(len) as usize, next(len) as usize);
        let a: Vec<u8> = (0..la).map(|_| next(alphabet) as u8).collect();
        let b: Vec<u8> = (0..lb).map(|_| next(alphabet) as u8).collect();
        let ops = diff(&a, &b);
        let (rebuilt, same) = apply(&a, &b, &ops);
        assert_eq!(rebuilt, b, "{a:?} -> {b:?}: {ops:?}");
        assert_eq!(same, lcs(&a, &b), "{a:?} -> {b:?}: {ops:?}");
    }
}

#[test]
fn many_scattered_edits_still_diff_line_by_line() {
    // Far past MAX_D edits: every seventh line changed across 30,000.
    let a: Vec<u32> = (0..30_000).collect();
    let b: Vec<u32> = (0..30_000).map(|i| if i % 7 == 3 { i + 1_000_000 } else { i }).collect();
    let ops = diff(&a, &b);
    let (rebuilt, same) = apply(&a, &b, &ops);
    assert_eq!(rebuilt, b);
    assert_eq!(same, 30_000 - 4_286, "only the changed lines differ");
}

#[test]
fn adversarial_inputs_stay_within_bounds_and_rebuild_the_new_side() {
    // All equal.
    let eq: Vec<u32> = (0..2_500).collect();
    assert_eq!(diff(&eq, &eq), vec![Op::Equal { old: 0, new: 0, len: 2_500 }]);

    // All different, both below and past MAX_D.
    for n in [50usize, 2_000] {
        let a: Vec<u32> = (0..n as u32).collect();
        let b: Vec<u32> = (n as u32..2 * n as u32).collect();
        let ops = diff(&a, &b);
        let (rebuilt, same) = apply(&a, &b, &ops);
        assert_eq!(rebuilt, b);
        assert_eq!(same, 0);
    }

    // Short side matching inside a much longer side (both within and past MAX_D).
    let short: Vec<u32> = (1..=5).collect();
    for len in [20u32, 2_500] {
        let long: Vec<u32> = (0..len).collect();
        let ops = diff(&short, &long);
        let (rebuilt, same) = apply(&short, &long, &ops);
        assert_eq!(rebuilt, long);
        assert_eq!(same, 5);

        let ops_rev = diff(&long, &short);
        let (rebuilt_rev, same_rev) = apply(&long, &short, &ops_rev);
        assert_eq!(rebuilt_rev, short);
        assert_eq!(same_rev, 5);
    }

    // Repeated identical blocks (classic Myers pathology).
    let mut a: Vec<u32> = (0..400).flat_map(|_| [1, 2, 3, 4]).collect();
    let mut b = a.clone();
    a.splice(800..804, [9, 9]);
    b.splice(400..400, [1, 2, 3, 4, 8, 8]);
    let ops = diff(&a, &b);
    let (rebuilt, same) = apply(&a, &b, &ops);
    assert_eq!(rebuilt, b);
    assert!(same >= 1_596);
}

#[test]
fn runs_coalesce_with_deletions_first() {
    let a = ["a", "b", "c", "d"];
    let b = ["a", "x", "y", "d", "e"];
    assert_eq!(
        diff(&a, &b),
        vec![
            Op::Equal { old: 0, new: 0, len: 1 },
            Op::Delete { old: 1, len: 2 },
            Op::Insert { new: 1, len: 2 },
            Op::Equal { old: 3, new: 3, len: 1 },
            Op::Insert { new: 4, len: 1 },
        ]
    );
    assert_eq!(diff::<u8>(&[], &[]), vec![]);
}

#[test]
fn word_changes_cover_changed_words_and_join_across_spaces() {
    use super::super::words::words;
    let (a, b) = words("let total = price * count;", "let total = price * amount + tax;");
    assert_eq!(a, Some(20..25));
    assert_eq!(&"let total = price * amount + tax;"[b.unwrap()], "amount + tax");
    assert_eq!(words("fn main() {}", "SELECT * FROM users"), (Some(0..12), Some(0..19)));
    assert_eq!(words("same", "same"), (None, None));
}
