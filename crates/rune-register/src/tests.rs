use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Item(&'static str, u32);

fn part(module: &'static str, items: &'static [Item]) -> Part<Item> {
    Part { module, items }
}

#[test]
fn orders_by_module_then_own_order() {
    let parts = [
        part("app::b", &[Item("b1", 0), Item("b2", 0)]),
        part("app::a", &[Item("a1", 0)]),
    ];
    let names: Vec<_> = collect(&parts).iter().map(|i| i.0).collect();
    assert_eq!(names, ["a1", "b1", "b2"]);
}

#[test]
fn empty_slice_is_empty() {
    assert!(collect::<Item>(&[]).is_empty());
}

#[test]
fn merge_joins_by_name_and_keeps_footer_last() {
    let own = [Item("one", 1), Item("two", 2), Item("footer", 9)];
    let more = [Item("two", 20), Item("new", 5)];
    let out = merge(&own, &more, |i| i.0, |o, m| o.1 += m.1);
    assert_eq!(out, [Item("one", 1), Item("two", 22), Item("new", 5), Item("footer", 9)]);
}

#[test]
fn merge_into_empty_own() {
    let out = merge(&[], &[Item("x", 1)], |i| i.0, |_, _| {});
    assert_eq!(out, [Item("x", 1)]);
}
