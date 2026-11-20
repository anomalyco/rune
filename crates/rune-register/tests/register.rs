#![allow(unsafe_code)]

use rune_register::{collect, register, slice};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Item(&'static str, u32);

slice!(SLICE: Item);

mod one {
    use super::{Item, SLICE, register};
    register!(SLICE: Item = [Item("z", 1), Item("y", 2)]);
}

mod two {
    use super::{Item, SLICE, register};
    register!(SLICE: Item = [Item("w", 3)]);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn registered_parts_are_collected_in_module_order() {
    let names: Vec<_> = collect(&SLICE).iter().map(|i| i.0).collect();
    assert_eq!(names, ["z", "y", "w"]);
}

#[cfg(target_family = "wasm")]
#[test]
fn wasm_lists_are_empty() {
    assert!(collect(&SLICE).is_empty());
}
