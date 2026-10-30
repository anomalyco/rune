//! TextInput, Textarea, Field and Select.

use super::*;

// TextInput, Textarea, Field
//
// Solid passes `defaultValue` to these inputs as an attribute the DOM ignores, so the web
// stories show their placeholders; the gallery does the same.

fn edit(v: &str) -> TextEdit {
    TextEdit::new(v)
}

fn grid(gap: f32, width: f32) -> El {
    col().gap(gap).w(width).items_stretch()
}

pub(super) fn text_input_playground(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(TextInput::new(&s.edits[0]).placeholder("Placeholder").on_edit(|s: &mut G| &mut s.edits[0]).build(k))
}

pub(super) fn text_input_copy(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(TextInput::new(&edit("")).placeholder("api.example.com/v1").copy_button().build(k))
}

pub(super) fn text_input_controlled(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let value = if s.edits[1].text.is_empty() && !s.is("typed") {
        "Controlled value".to_owned()
    } else {
        s.edits[1].text.clone()
    };
    col()
        .gap(12.0)
        .child(TextInput::new(&edit(&value)).placeholder("Type here…").build(k))
        .child(text(format!("Value: {value}")).font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.std_text))
}

pub(super) fn text_input_appearances(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(20.0, 280.0)
        .child(TextInput::new(&edit("")).placeholder("Base (28px)").build(k))
        .child(TextInput::new(&edit("")).placeholder("Large (32px)").large().build(k))
        .child(TextInput::new(&edit("")).placeholder("Large with copy").large().copy_button().build(k))
}

pub(super) fn text_input_with_field(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(24.0, 280.0)
        .child(
            Field::new()
                .label("Label")
                .info()
                .prefix("Prefix")
                .control(TextInput::new(&edit("")).placeholder("Text").copy_button().build(k))
                .suffix("Suffix")
                .build(k),
        )
        .child(
            Field::new()
                .label("Label")
                .prefix("Prefix")
                .control(TextInput::new(&edit("")).placeholder("Text").copy_button().invalid(true).build(k))
                .suffix("Suffix")
                .build(k),
        )
}

pub(super) fn text_input_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(20.0, 280.0)
        .child(TextInput::new(&edit("")).placeholder("Default").build(k))
        .child(TextInput::new(&edit("")).placeholder("With value").build(k))
        .child(TextInput::new(&edit("")).placeholder("With copy").copy_button().build(k))
        .child(TextInput::new(&edit("")).placeholder("Invalid").invalid(true).copy_button().build(k))
        .child(TextInput::new(&edit("")).placeholder("Disabled").disabled(true).build(k))
        .child(TextInput::new(&edit("")).placeholder("Disabled with value").disabled(true).copy_button().build(k))
}

pub(super) fn textarea_playground(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(TextArea::new(&s.edits[2]).placeholder("Placeholder").on_edit(|s: &mut G| &mut s.edits[2]).build(k))
}

pub(super) fn textarea_controlled(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(12.0, 280.0)
        .child(TextArea::new(&edit("")).placeholder("Type here…").build(k))
        .child(text("Value: Controlled value").font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.std_text))
}

pub(super) fn textarea_with_field(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(24.0, 280.0)
        .child(
            Field::new()
                .label("Label")
                .info()
                .prefix("Prefix")
                .control(TextArea::new(&edit("")).placeholder("Text").build(k))
                .suffix("Suffix")
                .build(k),
        )
        .child(
            Field::new()
                .label("Label")
                .prefix("Prefix")
                .control(TextArea::new(&edit("")).placeholder("Text").invalid(true).build(k))
                .suffix("Suffix")
                .build(k),
        )
}

pub(super) fn textarea_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(20.0, 280.0)
        .child(TextArea::new(&edit("")).placeholder("Default").build(k))
        .child(TextArea::new(&edit("")).placeholder("With value").build(k))
        .child(TextArea::new(&edit("")).placeholder("Invalid").invalid(true).build(k))
        .child(TextArea::new(&edit("")).placeholder("Disabled").disabled(true).build(k))
        .child(TextArea::new(&edit("")).placeholder("Disabled with value").disabled(true).build(k))
}

pub(super) fn field_text_input(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .info()
            .prefix("Prefix")
            .control(TextInput::new(&edit("")).placeholder("Text").copy_button().build(k))
            .suffix("Suffix")
            .build(k),
    )
}

pub(super) fn field_direct_child(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .prefix("Prefix")
            .control(TextInput::new(&edit("")).placeholder("Text").build(k))
            .suffix("Suffix")
            .build(k),
    )
}

pub(super) fn field_textarea(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .prefix("Prefix")
            .control(TextArea::new(&edit("")).placeholder("Text").build(k))
            .suffix("Suffix")
            .build(k),
    )
}

pub(super) fn field_invalid(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .prefix("Prefix")
            .control(TextInput::new(&edit("")).placeholder("Text").copy_button().invalid(true).build(k))
            .suffix("Suffix")
            .build(k),
    )
}

// Select

const FRUITS: [&str; 5] = ["Apple", "Banana", "Cherry", "Date", "Elderberry"];

fn fruit_select(s: &G, key: &'static str) -> Select {
    let current = s.pick.get(key).cloned();
    Select::new(key, &FRUITS)
        .placeholder("Pick a fruit")
        .current(current.as_deref())
        .open(is_open(s, key))
        .on_toggle(toggle(key))
        .on_close(close)
        .on_choose(move |s: &mut G, v: &str, _: &mut Tx| {
            s.pick.insert(key.to_owned(), v.to_owned());
            s.open = None;
        })
}

pub(super) fn select_playground(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(fruit_select(s, "select").build(k, layer))
}

pub(super) fn select_grouped(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let cities =
        [("North", "Boston"), ("South", "Miami"), ("South", "Atlanta"), ("West", "Seattle"), ("West", "Denver")];
    let current = s.pick.get("select").cloned();
    row().child(
        Select::grouped("select", &cities)
            .placeholder("Pick a fruit")
            .current(current.as_deref())
            .open(is_open(s, "select"))
            .on_toggle(toggle("select"))
            .on_close(close)
            .on_choose(|s: &mut G, v: &str, _: &mut Tx| {
                s.pick.insert("select".into(), v.to_owned());
                s.open = None;
            })
            .build(k, layer),
    )
}

pub(super) fn select_invalid(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(fruit_select(s, "select").invalid(true).build(k, layer))
}

pub(super) fn select_disabled(_: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(Select::new("select", &FRUITS).current(Some("Cherry")).disabled(true).build(k, layer))
}

pub(super) fn select_with_field(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Fruit")
            .info()
            .prefix("Optional helper")
            .control(fruit_select(s, "select").build(k, layer))
            .suffix("After selection")
            .build(k),
    )
}
