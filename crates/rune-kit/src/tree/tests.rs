use super::*;

fn model() -> TreeModel<&'static str> {
    let mut m = TreeModel::new();
    m.insert(None, "a", "alpha");
    m.insert(Some("a"), "a/1", "one");
    m.insert(Some("a"), "a/2", "two");
    m.insert(None, "b", "beta");
    m.insert(None, "c", "bravo");
    m
}

#[test]
fn typing_jumps_by_prefix_and_a_repeated_letter_cycles() {
    let mut m = model();
    fn label<'a>(t: &'a &'static str) -> &'a str {
        t
    }
    let r = m.type_select("b", true, label);
    assert_eq!(r, Some(1), "beta: rows are alpha, beta, bravo");
    m.select("b");
    assert_eq!(m.type_select("b", false, label), Some(2), "b again cycles to bravo");
    m.select("c");
    assert_eq!(m.type_select("r", false, label), None, "\"bbr\" is no name");
    assert_eq!(m.type_select("b", true, label), Some(1), "after a pause, a new prefix from the next row");
    m.select("a");
    assert_eq!(m.type_select("b", true, label), Some(1));
    assert_eq!(m.type_select("r", false, label), Some(2), "\"br\" keeps looking past beta to bravo");
}

#[test]
fn natural_order_counts_digits_and_ignores_case() {
    let mut v = vec!["file10", "File2", "file1", "a", "B"];
    v.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(v, ["a", "B", "file1", "File2", "file10"]);
}
