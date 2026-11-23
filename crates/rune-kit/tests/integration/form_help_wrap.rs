use rune::{
    Cx, El, Env, Mods, PhysicalInput, PhysicalKeyCode as Code, Runtime, ShortcutError, TextEdit,
    col, row,
};
use rune_kit::{
    Checkbox, Field, Kit, RadioGroup, RadioItem, ShortcutRecorder, ShortcutRecorderState, Switch,
    TextField, TextInput,
};

const SHORT: &str = "Short help.";
const LONG: &str = "This help sentence explains the setting in enough detail that it must wrap across multiple lines inside a narrow column.";

fn find_under<'a>(
    els: &'a [rune::runtime::ElementInfo],
    root_key: &str,
    child_key: &str,
) -> &'a rune::runtime::ElementInfo {
    let root = els
        .iter()
        .find(|e| e.key.as_deref() == Some(root_key))
        .unwrap_or_else(|| panic!("missing root {root_key}"));
    let prefix = format!("{}/", root.path);
    els.iter()
        .find(|e| e.path.starts_with(&prefix) && e.key.as_deref() == Some(child_key))
        .unwrap_or_else(|| panic!("missing {child_key} under {root_key}"))
}

fn by_key<'a>(
    els: &'a [rune::runtime::ElementInfo],
    key: &str,
) -> &'a rune::runtime::ElementInfo {
    els.iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key}"))
}

fn assert_wraps_inside(
    name: &str,
    container: [f32; 4],
    short_ctrl: [f32; 4],
    short_text: [f32; 4],
    long_ctrl: [f32; 4],
    long_text: [f32; 4],
) {
    assert!(
        short_text[3] > 0.0,
        "{name}: short text must have positive height: {short_text:?}"
    );
    assert!(
        long_text[0] >= container[0] - 0.5
            && long_text[0] + long_text[2] <= container[0] + container[2] + 0.5,
        "{name}: long text {long_text:?} overflows container {container:?}"
    );
    assert!(
        long_text[0] >= long_ctrl[0] - 0.5
            && long_text[0] + long_text[2] <= long_ctrl[0] + long_ctrl[2] + 0.5,
        "{name}: long text {long_text:?} overflows control {long_ctrl:?}"
    );
    assert!(
        long_ctrl[0] + long_ctrl[2] <= container[0] + container[2] + 0.5,
        "{name}: control {long_ctrl:?} overflows container {container:?}"
    );
    assert!(
        long_text[3] >= short_text[3] * 2.0 - 0.5,
        "{name}: long text should wrap onto multiple lines: {long_text:?} vs one line {short_text:?}"
    );
    assert!(
        long_text[1] + long_text[3] <= long_ctrl[1] + long_ctrl[3] + 0.5,
        "{name}: control {long_ctrl:?} does not enclose wrapped text {long_text:?}"
    );
    assert!(
        long_ctrl[3] > short_ctrl[3] + short_text[3] - 0.5,
        "{name}: control height did not grow for wrapped text: long {long_ctrl:?} vs short {short_ctrl:?} (line {})",
        short_text[3]
    );
}

#[test]
fn field_prefix_and_suffix_wrap_in_narrow_container() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let edit = TextEdit::new("");
        let mk = |help: &'static str, tag: &str| {
            col().key(format!("box-{tag}")).w(240.0).child(
                Field::new()
                    .label("Host")
                    .prefix(help)
                    .control(TextInput::new(&edit).width(None).placeholder("host").build(&k).key("ctrl"))
                    .suffix(help)
                    .build(&k)
                    .key(format!("field-{tag}")),
            )
        };
        row().gap(16.0).child(mk(SHORT, "short")).child(mk(LONG, "long"))
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let long_box = by_key(&els, "box-long").rect;
    let sc = by_key(&els, "field-short").rect;
    let lc = by_key(&els, "field-long").rect;
    let sp = find_under(&els, "field-short", "prefix").rect;
    let lp = find_under(&els, "field-long", "prefix").rect;
    let ss = find_under(&els, "field-short", "suffix").rect;
    let ls = find_under(&els, "field-long", "suffix").rect;
    let ctrl = find_under(&els, "field-long", "ctrl").rect;

    assert_wraps_inside("Field::prefix", long_box, sc, sp, lc, lp);
    assert_wraps_inside("Field::suffix", long_box, sc, ss, lc, ls);
    assert!(lp[1] + lp[3] <= ctrl[1] + 0.5, "prefix {lp:?} overlaps control {ctrl:?}");
    assert!(ctrl[1] + ctrl[3] <= ls[1] + 0.5, "control {ctrl:?} overlaps suffix {ls:?}");
}

#[test]
fn text_field_error_wraps_in_narrow_container() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let edit = TextEdit::new("");
        let mk = |err: &'static str, tag: &str| {
            col().key(format!("box-{tag}")).w(240.0).child(
                TextField::new("Name", &edit)
                    .error(err)
                    .build(&k)
                    .key(format!("tf-{tag}")),
            )
        };
        row().gap(16.0).child(mk(SHORT, "short")).child(mk(LONG, "long"))
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let long_box = by_key(&els, "box-long").rect;
    let sc = by_key(&els, "tf-short").rect;
    let lc = by_key(&els, "tf-long").rect;
    let se = find_under(&els, "tf-short", "error").rect;
    let le = find_under(&els, "tf-long", "error").rect;
    let wrapper = find_under(&els, "tf-long", "wrapper").rect;

    assert_wraps_inside("TextField::error", long_box, sc, se, lc, le);
    assert!(wrapper[1] + wrapper[3] <= le[1] + 0.5, "wrapper {wrapper:?} overlaps error {le:?}");
}

#[test]
fn switch_description_wraps_in_narrow_container() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mk = |desc: &'static str, tag: &str| {
            col().key(format!("box-{tag}")).w(240.0).child(
                Switch::new(false)
                    .label("Alerts")
                    .description(desc)
                    .build(&k)
                    .key(format!("sw-{tag}")),
            )
        };
        row().gap(16.0).child(mk(SHORT, "short")).child(mk(LONG, "long"))
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let long_box = by_key(&els, "box-long").rect;
    let sc = by_key(&els, "sw-short").rect;
    let lc = by_key(&els, "sw-long").rect;
    let sd = find_under(&els, "sw-short", "description").rect;
    let ld = find_under(&els, "sw-long", "description").rect;
    let ctrl = find_under(&els, "sw-long", "control").rect;

    assert_wraps_inside("Switch::description", long_box, sc, sd, lc, ld);
    assert!(ld[0] + ld[2] <= ctrl[0] + 0.5, "description {ld:?} overlaps control {ctrl:?}");
    assert!(
        ctrl[0] + ctrl[2] <= long_box[0] + long_box[2] + 0.5,
        "control {ctrl:?} pushed outside container {long_box:?}"
    );
}

#[test]
fn checkbox_description_wraps_in_narrow_container() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mk = |desc: &'static str, tag: &str| {
            col().key(format!("box-{tag}")).w(240.0).child(
                Checkbox::new(false)
                    .label("Sync")
                    .description(desc)
                    .build(&k)
                    .key(format!("cb-{tag}")),
            )
        };
        row().gap(16.0).child(mk(SHORT, "short")).child(mk(LONG, "long"))
    }

    let mut rt = Runtime::new((), view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let long_box = by_key(&els, "box-long").rect;
    let sc = by_key(&els, "cb-short").rect;
    let lc = by_key(&els, "cb-long").rect;
    let sd = find_under(&els, "cb-short", "description").rect;
    let ld = find_under(&els, "cb-long", "description").rect;
    let label = find_under(&els, "cb-long", "label").rect;
    let ctrl = find_under(&els, "cb-long", "control").rect;

    assert_wraps_inside("Checkbox::description", long_box, sc, sd, lc, ld);
    assert!(label[1] + label[3] <= ld[1] + 0.5, "label {label:?} overlaps description {ld:?}");
    assert!(ctrl[0] + ctrl[2] <= ld[0] + 0.5, "control {ctrl:?} overlaps description {ld:?}");
}

#[test]
fn radio_item_and_group_descriptions_wrap_in_narrow_container() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mk = |desc: &'static str, tag: &str| {
            col()
                .key(format!("box-{tag}"))
                .w(240.0)
                .gap(12.0)
                .child(
                    RadioItem::new("Dark", true)
                        .description(desc)
                        .build(&k)
                        .key(format!("ri-{tag}")),
                )
                .child(
                    RadioGroup::new()
                        .label("Theme")
                        .description(desc)
                        .item(RadioItem::new("Light", true).description(desc))
                        .build(&k)
                        .key(format!("rg-{tag}")),
                )
        };
        row().gap(16.0).child(mk(SHORT, "short")).child(mk(LONG, "long"))
    }

    let mut rt = Runtime::new((), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let long_box = by_key(&els, "box-long").rect;

    // Standalone RadioItem.
    let sc = by_key(&els, "ri-short").rect;
    let lc = by_key(&els, "ri-long").rect;
    let sd = find_under(&els, "ri-short", "description").rect;
    let ld = find_under(&els, "ri-long", "description").rect;
    let label = find_under(&els, "ri-long", "label").rect;
    let stack = find_under(&els, "ri-long", "stack").rect;
    assert_wraps_inside("RadioItem::description", long_box, sc, sd, lc, ld);
    assert!(label[1] + label[3] <= ld[1] + 0.5, "label {label:?} overlaps description {ld:?}");
    assert!(stack[0] + stack[2] <= ld[0] + 0.5, "dot {stack:?} overlaps description {ld:?}");

    // RadioGroup description and nested RadioItem description.
    let gsc = by_key(&els, "rg-short").rect;
    let glc = by_key(&els, "rg-long").rect;
    let gsd = find_under(&els, "rg-short", "description").rect;
    let gld = find_under(&els, "rg-long", "description").rect;
    let items = find_under(&els, "rg-long", "items").rect;
    assert_wraps_inside("RadioGroup::description", long_box, gsc, gsd, glc, gld);
    assert!(gld[1] + gld[3] <= items[1] + 0.5, "group description {gld:?} overlaps items {items:?}");

    let item0 = find_under(&els, "rg-long", "item0");
    let item_desc = els
        .iter()
        .find(|e| e.path.starts_with(&format!("{}/", item0.path)) && e.key.as_deref() == Some("description"))
        .unwrap()
        .rect;
    assert!(
        item_desc[0] + item_desc[2] <= long_box[0] + long_box[2] + 0.5,
        "group item description {item_desc:?} overflows container {long_box:?}"
    );
    assert!(item_desc[3] >= sd[3] * 2.0 - 0.5, "group item description wraps: {item_desc:?}");
}

#[test]
fn shortcut_recorder_error_wraps_inside_recorder_width() {
    struct RecState {
        recorder: ShortcutRecorderState,
        msg: &'static str,
        width: Option<f32>,
    }

    fn view(s: &RecState, _: &mut Cx<RecState>) -> El {
        let k = Kit::neutral(true);
        let msg = s.msg;
        let mut rec = ShortcutRecorder::new("rec", &s.recorder, |s: &mut RecState| &mut s.recorder)
            .error_message(move |_, _| msg.to_owned())
            .on_change(|_, _, _| Err(ShortcutError::Conflict));
        if let Some(w) = s.width {
            rec = rec.width(w);
        }
        col().items_start().child(col().key("box").w(240.0).child(rec.build(&k)))
    }

    let run = |msg: &'static str, width: Option<f32>| {
        let mut rt = Runtime::new(
            RecState {
                recorder: ShortcutRecorderState::new(Some("ctrl+k".parse().unwrap())),
                msg,
                width,
            },
            view,
            (400.0, 300.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let (x, y) = rt.find("rec").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        rt.physical_input(PhysicalInput::Key {
            key: Code::KeyJ,
            mods: Mods { ctrl: true, ..Mods::default() },
            pressed: true,
            repeat: false,
        });
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        let els = rt.elements();
        (
            by_key(&els, "box").rect,
            by_key(&els, "rec-field").rect,
            find_under(&els, "rec-field", "rec").rect,
            find_under(&els, "rec-field", "error").rect,
        )
    };

    let (_, sc, _, se) = run(SHORT, None);
    let (long_box, lc, btn, le) = run(LONG, None);
    assert_wraps_inside("ShortcutRecorder::error", long_box, sc, se, lc, le);
    assert!(btn[1] + btn[3] <= le[1] + 0.5, "button {btn:?} overlaps error {le:?}");

    let (_, nc, nbtn, ne) = run(LONG, Some(180.0));
    assert!((nc[2] - 180.0).abs() < 0.5, "custom width 180 preserved: {nc:?}");
    assert!(ne[0] + ne[2] <= nc[0] + nc[2] + 0.5, "narrow error {ne:?} overflows field {nc:?}");
    assert!(ne[3] >= se[3] * 2.0 - 0.5, "narrow error wraps: {ne:?}");
    assert!(nbtn[1] + nbtn[3] <= ne[1] + 0.5, "narrow button {nbtn:?} overlaps error {ne:?}");
    assert!(ne[1] + ne[3] <= nc[1] + nc[3] + 0.5, "narrow field {nc:?} encloses error {ne:?}");
}

#[test]
fn short_help_text_stays_single_line_and_preserves_layout() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let edit = TextEdit::new("");
        col()
            .items_start()
            .child(
                col()
                    .key("wide")
                    .w(400.0)
                    .gap(12.0)
                    .child(
                        Field::new()
                            .label("Host")
                            .prefix(SHORT)
                            .control(TextInput::new(&edit).width(None).build(&k))
                            .suffix(SHORT)
                            .build(&k)
                            .key("field"),
                    )
                    .child(TextField::new("Name", &edit).error(SHORT).build(&k).key("textfield"))
                    .child(
                        row()
                            .key("inline-row")
                            .gap(12.0)
                            .child(Switch::new(false).label("Alerts").description(SHORT).build(&k).key("switch"))
                            .child(Checkbox::new(false).label("Sync").description(SHORT).build(&k).key("checkbox")),
                    )
                    .child(
                        row()
                            .key("radio-row")
                            .gap(12.0)
                            .child(RadioItem::new("Dark", true).description(SHORT).build(&k).key("radioitem"))
                            .child(
                                RadioGroup::new()
                                    .label("Theme")
                                    .description(SHORT)
                                    .item(RadioItem::new("Light", true).description(SHORT))
                                    .build(&k)
                                    .key("radiogroup"),
                            ),
                    ),
            )
    }

    let mut rt = Runtime::new((), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();

    let field_prefix = find_under(&els, "field", "prefix").rect;
    let field_suffix = find_under(&els, "field", "suffix").rect;
    let tf_error = find_under(&els, "textfield", "error").rect;
    let sw = by_key(&els, "switch").rect;
    let sw_desc = find_under(&els, "switch", "description").rect;
    let cb = by_key(&els, "checkbox").rect;
    let cb_desc = find_under(&els, "checkbox", "description").rect;
    let ri = by_key(&els, "radioitem").rect;
    let ri_desc = find_under(&els, "radioitem", "description").rect;
    let rg = by_key(&els, "radiogroup").rect;
    let rg_desc = find_under(&els, "radiogroup", "description").rect;

    // Every short help/error line stays on one line with its configured line height.
    assert!((field_prefix[3] - 11.0).abs() < 0.5, "Field prefix: {field_prefix:?}");
    assert!((field_suffix[3] - 11.0).abs() < 0.5, "Field suffix: {field_suffix:?}");
    assert!(tf_error[3] > 0.0 && tf_error[3] < 24.0, "TextField error: {tf_error:?}");
    assert!((sw_desc[3] - 15.6).abs() < 0.6, "Switch description: {sw_desc:?}");
    assert!((cb_desc[3] - 15.6).abs() < 0.6, "Checkbox description: {cb_desc:?}");
    assert!((ri_desc[3] - 11.0).abs() < 0.5, "RadioItem description: {ri_desc:?}");
    assert!((rg_desc[3] - 13.2).abs() < 0.6, "RadioGroup description: {rg_desc:?}");

    // Short inline controls keep their natural fit-content widths inside horizontal rows.
    assert!(sw[2] < 200.0 && cb[2] < 200.0, "Switch {sw:?} and Checkbox {cb:?} stay compact");
    assert!(ri[2] < 200.0 && rg[2] < 200.0, "RadioItem {ri:?} and RadioGroup {rg:?} stay compact");
}

#[test]
fn inline_controls_wrap_inside_narrow_field() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mk = |desc: &'static str, tag: &str| {
            col()
                .key(format!("box-{tag}"))
                .w(240.0)
                .gap(12.0)
                .child(
                    Field::new()
                        .label("Switch")
                        .control(Switch::new(false).label("Alerts").description(desc).build(&k).key(format!("sw-{tag}")))
                        .build(&k),
                )
                .child(
                    Field::new()
                        .label("Checkbox")
                        .control(Checkbox::new(false).label("Sync").description(desc).build(&k).key(format!("cb-{tag}")))
                        .build(&k),
                )
                .child(
                    Field::new()
                        .label("RadioItem")
                        .control(RadioItem::new("Dark", true).description(desc).build(&k).key(format!("ri-{tag}")))
                        .build(&k),
                )
                .child(
                    Field::new()
                        .label("RadioGroup")
                        .control(
                            RadioGroup::new()
                                .label("Theme")
                                .description(desc)
                                .item(RadioItem::new("Light", true).description(desc))
                                .build(&k)
                                .key(format!("rg-{tag}")),
                        )
                        .build(&k),
                )
        };
        row().gap(16.0).child(mk(SHORT, "short")).child(mk(LONG, "long"))
    }

    let mut rt = Runtime::new((), view, (800.0, 1200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let long_box = by_key(&els, "box-long").rect;
    for (prefix, name) in [
        ("sw", "Field(Switch)"),
        ("cb", "Field(Checkbox)"),
        ("ri", "Field(RadioItem)"),
        ("rg", "Field(RadioGroup)"),
    ] {
        let sc = by_key(&els, &format!("{prefix}-short")).rect;
        let lc = by_key(&els, &format!("{prefix}-long")).rect;
        let sd = find_under(&els, &format!("{prefix}-short"), "description").rect;
        let ld = find_under(&els, &format!("{prefix}-long"), "description").rect;
        assert_wraps_inside(name, long_box, sc, sd, lc, ld);
    }
}
