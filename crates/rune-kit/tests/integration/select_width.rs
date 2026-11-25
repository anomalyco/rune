use rune::{Cx, El, Env, Runtime, col, row, text};
use rune_kit::{Field, Kit, Placement, Select, SelectAppearance, root};

#[derive(Clone, Copy)]
enum Container {
    Column(f32),
    UnstretchedColumn(f32),
    PaddedColumn { w: f32, pad: f32 },
    RowWithLabel { w: f32, label_w: f32, gap: f32 },
    FormField(f32),
    Pinned { x: f32, y: f32, w: f32 },
}

#[derive(Clone, Copy)]
struct Spec {
    appearance: SelectAppearance,
    width: Option<f32>,
    fill: bool,
    width_after_fill: bool,
    menu_width: Option<f32>,
    placement: Placement,
    container: Container,
    window: (f32, f32),
    long_current: bool,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            appearance: SelectAppearance::Field,
            width: None,
            fill: false,
            width_after_fill: false,
            menu_width: None,
            placement: Placement::BottomEnd,
            container: Container::Column(420.0),
            window: (1000.0, 800.0),
            long_current: false,
        }
    }
}

const LONG_LABEL: &str = "An extraordinarily long option label that must truncate inside a narrow trigger";

fn view(spec: &Spec, _: &mut Cx<Spec>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let current = if spec.long_current { LONG_LABEL } else { "A" };
    let mut s = Select::new("choice", &["A", "A much longer option label", LONG_LABEL])
        .appearance(spec.appearance)
        .placement(spec.placement)
        .current(Some(current))
        .open(true)
        .on_close(|_: &mut Spec| {});
    if spec.width_after_fill {
        if spec.fill {
            s = s.fill();
        }
        if let Some(w) = spec.width {
            s = s.width(w);
        }
    } else {
        if let Some(w) = spec.width {
            s = s.width(w);
        }
        if spec.fill {
            s = s.fill();
        }
    }
    if let Some(w) = spec.menu_width {
        s = s.menu_width(w);
    }
    let select = s.build(&k, &mut layer);
    let body = match spec.container {
        Container::Column(w) => col().key("host").w(w).child(select),
        Container::UnstretchedColumn(w) => col().key("host").w(w).items_start().child(select),
        Container::PaddedColumn { w, pad } => col().key("host").w(w).padding(pad).child(select),
        Container::RowWithLabel { w, label_w, gap } => row()
            .key("host")
            .w(w)
            .gap(gap)
            .center_y()
            .child(text("Theme").w(label_w))
            .child(select),
        Container::FormField(w) => col()
            .key("host")
            .w(w)
            .child(Field::new().label("Theme").control(select).build(&k)),
        Container::Pinned { x, y, w } => col().key("host").absolute(x, y).w(w).child(select),
    };
    root(rune::el().w_full().h_full().child(body), layer)
}

fn run(spec: Spec) -> Runtime<Spec> {
    let mut rt = Runtime::new(spec, view, spec.window, Env::new(true, 1.0));
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

fn rect(rt: &Runtime<Spec>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key}")).rect
}

#[test]
fn a_field_select_keeps_280_and_explicit_width_unless_fill_is_set() {
    let default_rt = run(Spec::default());
    assert_eq!(rect(&default_rt, "choice-trigger")[2], 280.0);

    let sized_rt = run(Spec { width: Some(200.0), ..Spec::default() });
    assert_eq!(rect(&sized_rt, "choice-trigger")[2], 200.0);

    let fill_after_width = run(Spec { width: Some(180.0), fill: true, ..Spec::default() });
    assert_eq!(rect(&fill_after_width, "choice-trigger")[2], 420.0);

    let width_after_fill = run(Spec { width: Some(180.0), fill: true, width_after_fill: true, ..Spec::default() });
    assert_eq!(rect(&width_after_fill, "choice-trigger")[2], 420.0);
}

#[test]
fn fill_sizes_to_column_padding_row_and_truncates_without_overflow() {
    let col_rt = run(Spec { fill: true, container: Container::Column(420.0), ..Spec::default() });
    assert_eq!(rect(&col_rt, "choice-trigger")[2], 420.0);

    let pad_rt = run(Spec { fill: true, container: Container::PaddedColumn { w: 420.0, pad: 16.0 }, ..Spec::default() });
    assert_eq!(rect(&pad_rt, "choice-trigger")[2], 388.0);

    let field_rt = run(Spec { fill: true, container: Container::FormField(320.0), ..Spec::default() });
    assert_eq!(rect(&field_rt, "choice-trigger")[2], 320.0);

    let row_rt = run(Spec {
        fill: true,
        container: Container::RowWithLabel { w: 360.0, label_w: 80.0, gap: 12.0 },
        ..Spec::default()
    });
    let host = rect(&row_rt, "host");
    let trig = rect(&row_rt, "choice-trigger");
    assert_eq!(trig[2], 268.0, "trigger takes the row's remaining width: {trig:?}");
    assert!(trig[0] + trig[2] <= host[0] + host[2] + 0.5, "trigger {trig:?} overflows row {host:?}");

    let narrow_rt = run(Spec {
        fill: true,
        long_current: true,
        container: Container::Column(140.0),
        ..Spec::default()
    });
    let trig = rect(&narrow_rt, "choice-trigger");
    let val = rect(&narrow_rt, "value");
    let chev = rect(&narrow_rt, "chevron");
    assert_eq!(trig[2], 140.0, "long label must not push narrow filled trigger wider: {trig:?}");
    assert!(val[0] + val[2] <= chev[0] + 0.5, "truncated value {val:?} must stay clear of chevron {chev:?}");
    assert!(chev[0] + chev[2] <= trig[0] + trig[2] + 0.5, "chevron {chev:?} must stay inside trigger {trig:?}");
}

#[test]
fn menu_width_sets_listbox_width_and_preserves_clamping_alignment_and_flip() {
    let auto_rt = run(Spec::default());
    let auto = rect(&auto_rt, "choice-content")[2];
    assert!((160.0..=368.0).contains(&auto), "auto listbox width must stay in 160..=368, got {auto}");

    let wide_rt = run(Spec { fill: true, menu_width: Some(420.0), ..Spec::default() });
    assert_eq!(rect(&wide_rt, "choice-content")[2], 420.0);

    let narrow_rt = run(Spec { menu_width: Some(120.0), ..Spec::default() });
    assert_eq!(rect(&narrow_rt, "choice-content")[2], 120.0);

    let clamped_rt = run(Spec { menu_width: Some(1200.0), window: (500.0, 400.0), ..Spec::default() });
    let clamped = rect(&clamped_rt, "choice-content");
    assert_eq!(clamped[2], 484.0, "listbox wider than window clamps to viewport minus 16pt margins: {clamped:?}");
    assert!(clamped[0] >= 8.0 && clamped[0] + clamped[2] <= 492.0 + 0.5, "clamped listbox stays in window: {clamped:?}");

    let aligned_rt = run(Spec {
        fill: true,
        menu_width: Some(260.0),
        placement: Placement::BottomStart,
        container: Container::Pinned { x: 40.0, y: 40.0, w: 260.0 },
        window: (800.0, 600.0),
        ..Spec::default()
    });
    let trig = rect(&aligned_rt, "choice-trigger");
    let menu = rect(&aligned_rt, "choice-content");
    assert_eq!(menu[0], trig[0], "BottomStart menu aligns with trigger left edge: menu={menu:?} trig={trig:?}");
    assert_eq!(menu[2], 260.0);
    assert!(menu[1] >= trig[1] + trig[3], "opens below trigger when room exists: menu={menu:?} trig={trig:?}");

    let flipped_rt = run(Spec {
        fill: true,
        menu_width: Some(260.0),
        placement: Placement::BottomStart,
        container: Container::Pinned { x: 40.0, y: 550.0, w: 260.0 },
        window: (800.0, 600.0),
        ..Spec::default()
    });
    let trig = rect(&flipped_rt, "choice-trigger");
    let menu = rect(&flipped_rt, "choice-content");
    assert_eq!(menu[0], trig[0], "flipped menu stays left-aligned with trigger: menu={menu:?} trig={trig:?}");
    assert!(menu[1] + menu[3] <= trig[1], "near bottom edge, menu flips above trigger: menu={menu:?} trig={trig:?}");
}

#[test]
fn inline_select_ignores_fill_but_honours_menu_width() {
    let base = run(Spec {
        appearance: SelectAppearance::Inline,
        fill: false,
        container: Container::UnstretchedColumn(420.0),
        ..Spec::default()
    });
    let filled = run(Spec {
        appearance: SelectAppearance::Inline,
        fill: true,
        menu_width: Some(310.0),
        container: Container::UnstretchedColumn(420.0),
        ..Spec::default()
    });
    let base_trig = rect(&base, "choice-trigger");
    let trig = rect(&filled, "choice-trigger");
    let menu = rect(&filled, "choice-content");
    assert_eq!(trig[2], base_trig[2], "Inline trigger ignores fill(): {trig:?} vs {base_trig:?}");
    assert!(trig[2] < 200.0, "Inline trigger stays fit-content: {trig:?}");
    assert_eq!(menu[2], 310.0, "Inline listbox still honours menu_width(): {menu:?}");
}

#[test]
fn select_listbox_keeps_160_min_width_regardless_of_min_w_and_shrink_order() {
    // Select with short options keeps its 160 pt listbox floor even on a narrow trigger.
    let select_rt = run(Spec {
        appearance: SelectAppearance::Inline,
        width: Some(60.0),
        container: Container::UnstretchedColumn(420.0),
        ..Spec::default()
    });
    assert!(rect(&select_rt, "choice-content")[2] >= 160.0);

    // Both builder orders on a listbox-shaped column produce the exact same 160 pt minimum width.
    let mut rt = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            col().size(400.0, 200.0)
                .child(row().w(80.0).child(
                    col().key("min_then_shrink").min_h(0.0).min_w(160.0).max_w(368.0).shrink().child(text("A")),
                ))
                .child(row().w(80.0).child(
                    col().key("shrink_then_min").min_h(0.0).shrink().min_w(160.0).max_w(368.0).child(text("A")),
                ))
        },
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(rt.presented("min_then_shrink").unwrap().0[2], 160.0);
    assert_eq!(rt.presented("shrink_then_min").unwrap().0[2], 160.0);
}
