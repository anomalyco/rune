use rune::{Cx, El, Env, Runtime, col};
use rune_kit::{Kit, Select, SelectAppearance, root};

/// `(fill, menu_width)`.
type Sizes = (bool, Option<f32>);

fn view(&(fill, menu): &Sizes, _: &mut Cx<Sizes>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let mut s = Select::new("choice", &["A", "A much longer option label"])
        .appearance(SelectAppearance::Field)
        .current(Some("A"))
        .open(true)
        .on_close(|_: &mut Sizes| {});
    if fill {
        s = s.fill();
    }
    if let Some(w) = menu {
        s = s.menu_width(w);
    }
    let select = s.build(&k, &mut layer);
    root(rune::el().w_full().h_full().child(col().key("column").w(420.0).child(select)), layer)
}

fn rect(fill: bool, menu: Option<f32>, key: &str) -> [f32; 4] {
    let mut rt = Runtime::new((fill, menu), view, (1000.0, 800.0), Env::new(true, 1.0));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
}

#[test]
fn a_field_select_is_280_wide_unless_it_fills_its_column() {
    assert_eq!(rect(false, None, "choice-trigger")[2], 280.0);
    assert_eq!(rect(true, None, "choice-trigger")[2], 420.0);
}

#[test]
fn menu_width_sets_the_listbox_width() {
    let auto = rect(false, None, "choice-content")[2];
    assert!((160.0..=368.0).contains(&auto), "auto width {auto}");
    assert_eq!(rect(true, Some(420.0), "choice-content")[2], 420.0);
}
