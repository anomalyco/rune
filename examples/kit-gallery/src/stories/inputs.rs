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

/// Read-only `TextInput` and `TextArea` (`read_only(true)`): focusable with visible caret,
/// keyboard navigation (arrows, Shift+arrows, Home/End, word steps), pointer selection, and Copy,
/// while refusing edits, Cut, Paste, Undo/Redo, and IME.
pub(super) fn text_input_read_only(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let status = s
        .pick
        .get("ro-status")
        .map_or("Arrows, Shift+arrows, Home/End, Cmd/Ctrl+A and Cmd/Ctrl+C work; typing, Cut and Paste are blocked.", String::as_str);
    let report = |name: &'static str, idx: usize| {
        move |s: &mut G, _: &mut Tx| {
            let e = &s.read_only_edits[idx];
            let (a, b) = e.range();
            let msg = if a == b {
                format!("{name}: caret at byte {a}")
            } else {
                format!("{name}: selected {a}..{b} ({:?})", e.selected())
            };
            s.pick.insert("ro-status".into(), msg);
        }
    };
    grid(16.0, 360.0)
        .child(
            Field::new()
                .label("Read-only TextInput")
                .control(
                    TextInput::new(&s.read_only_edits[0])
                        .label("Endpoint URL")
                        .read_only(true)
                        .copy_button()
                        .width(None)
                        .on_edit(|s: &mut G| &mut s.read_only_edits[0])
                        .on_input_selection(report("TextInput", 0))
                        .on_action(|s: &mut G, tx: &mut Tx| {
                            tx.copy(s.read_only_edits[0].text.clone());
                            s.pick.insert("ro-status".into(), "Copied full endpoint URL".into());
                        })
                        .build(k)
                        .key("ro-input"),
                )
                .build(k),
        )
        .child(
            Field::new()
                .label("Read-only TextArea")
                .control(
                    TextArea::new(&s.read_only_edits[1])
                        .label("Source preview")
                        .read_only(true)
                        .rows(4)
                        .width(None)
                        .on_edit(|s: &mut G| &mut s.read_only_edits[1])
                        .on_input_selection(report("TextArea", 1))
                        .build(k)
                        .key("ro-area"),
                )
                .build(k),
        )
        .child(text(status).key("ro-status").font(k.ty.small).color(k.c.text_muted).wrap())
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

/// `TextArea::plain()`, `TextArea::trailing(El)`, and `TextArea::scroll_bar(bool)` /
/// `TextArea::scroll_bar_styled(ScrollBarStyle)`: a borderless chat composer inside a custom
/// container, a commit box with a trailing button beside an auto-growing viewport, and a capped
/// field with a styled scroll bar.
pub(super) fn textarea_composer(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let status = s
        .pick
        .get("composer-status")
        .map_or("Try typing, scrolling the viewports, or tabbing to Send / Commit.", String::as_str);
    let styled_bar = ScrollBarStyle::from_kit(k)
        .always_visible()
        .thumb(k.c.text_accent)
        .thumb_hover(k.c.border_focus)
        .track(k.c.layer_02);
    grid(16.0, 340.0)
        .child(
            col()
                .key("plain-composer")
                .role("textbox")
                .w_full()
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_base)
                .radius(k.radius.lg)
                .child(
                    TextArea::new(&s.edits[8])
                        .label("Message")
                        .placeholder("Ask anything…")
                        .plain()
                        .width(None)
                        .auto_grow(1, 4)
                        .scroll_bar(true)
                        .on_edit(|s: &mut G| &mut s.edits[8])
                        .build(k)
                        .key("composer-plain"),
                )
                .child(
                    row()
                        .w_full()
                        .px(8.0)
                        .pb(8.0)
                        .gap(8.0)
                        .center_y()
                        .child(Badge::new("Plain composer").build(k))
                        .child(el().grow(1.0))
                        .child(
                            Button::new("Send")
                                .control_size(ControlSize::Small)
                                .variant(ButtonVariant::Contrast)
                                .on_press(|s: &mut G| {
                                    let len = s.edits[8].text.len();
                                    s.pick.insert("composer-status".into(), format!("Sent plain draft ({len} bytes)"));
                                })
                                .build(k)
                                .key("composer-send"),
                        ),
                ),
        )
        .child(
            Field::new()
                .label("Trailing action")
                .control(
                    TextArea::new(&s.edits[9])
                        .label("Commit message")
                        .placeholder("Commit message…")
                        .width(None)
                        .auto_grow(1, 3)
                        .scroll_bar(true)
                        .trailing(
                            Button::new("Commit")
                                .control_size(ControlSize::Small)
                                .variant(ButtonVariant::Submit)
                                .on_press(|s: &mut G| {
                                    let len = s.edits[9].text.len();
                                    s.pick.insert("composer-status".into(), format!("Committed ({len} bytes)"));
                                })
                                .build(k)
                                .key("composer-commit"),
                        )
                        .on_edit(|s: &mut G| &mut s.edits[9])
                        .build(k)
                        .key("composer-trailing"),
                )
                .build(k),
        )
        .child(
            Field::new()
                .label("Styled scroll bar")
                .control(
                    TextArea::new(&s.edits[10])
                        .label("Release notes")
                        .width(None)
                        .max_height(88.0)
                        .scroll_bar_styled(styled_bar)
                        .on_edit(|s: &mut G| &mut s.edits[10])
                        .build(k)
                        .key("composer-styled"),
                )
                .build(k),
        )
        .child(text(status).key("composer-status").font(k.ty.small).color(k.c.text_muted).wrap())
}

/// `TextArea::line_home_end(bool)` and `Shift+Insert` paste: opt-in visual line Home/End for
/// macOS code editors alongside standard Shift+Insert paste on Linux and Windows.
pub(super) fn textarea_code_keys(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let line_home_end = !s.is("code-keys-native");
    let edit = &s.edits[11];
    let mode = if line_home_end { "line_home_end(true)" } else { "line_home_end(false) (native macOS)" };
    let status = format!(
        "Mode: {mode} · Caret: {}..{} · Shift+Insert pastes on Linux/Windows.",
        edit.anchor, edit.focus
    );
    grid(12.0, 340.0)
        .child(
            Checkbox::new(line_home_end)
                .label("Line-based Home / End on macOS")
                .on_toggle(flip("code-keys-native"))
                .build(k)
                .key("code-keys-toggle"),
        )
        .child(
            Field::new()
                .label("Code editor")
                .control(
                    TextArea::new(edit)
                        .label("Code editor")
                        .description("Home and End jump to visual line edges when enabled; Cmd+Home/End jump to document ends")
                        .width(None)
                        .rows(4)
                        .line_home_end(line_home_end)
                        .on_edit(|s: &mut G| &mut s.edits[11])
                        .build(k)
                        .key("code-keys-area"),
                )
                .build(k),
        )
        .child(text(status).key("code-keys-status").font(k.ty.small).color(k.c.text_muted).wrap())
}

/// `TextArea::reveal_caret_on_focus(bool)` (`El::reveal_caret_on_focus`): tall scrolled
/// multi-line editor whose offscreen caret scrolls into view when focused via Tab or `Tx::focus`.
pub(super) fn textarea_focus_caret(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let reveal = !s.is("focus-caret-off");
    let edit = &s.focus_caret_edit;
    let mode = if reveal { "reveal_caret_on_focus(true)" } else { "reveal_caret_on_focus(false) (default)" };
    let status = format!(
        "Mode: {mode} · Caret byte: {} · Press Tab from the buttons or click Focus editor.",
        edit.focus
    );
    grid(12.0, 360.0)
        .child(
            Checkbox::new(reveal)
                .label("Reveal caret on focus")
                .on_toggle(flip("focus-caret-off"))
                .build(k)
                .key("focus-caret-toggle"),
        )
        .child(
            row()
                .gap(8.0)
                .child(
                    Button::new("Scroll to top")
                        .on_press_tx(|_: &mut G, tx: &mut Tx| {
                            tx.blur();
                            tx.scroll_to_y("focus-caret-area/scroll", 0.0);
                        })
                        .build(k)
                        .key("focus-caret-reset"),
                )
                .child(
                    Button::new("Focus editor")
                        .on_press_tx(|_: &mut G, tx: &mut Tx| {
                            tx.focus("focus-caret-area", true);
                        })
                        .build(k)
                        .key("focus-caret-btn"),
                ),
        )
        .child(
            Field::new()
                .label("Release log")
                .control(
                    TextArea::new(edit)
                        .label("Release log")
                        .description("Caret starts at line 30; Tab or Focus editor reveals it when enabled")
                        .width(None)
                        .max_height(140.0)
                        .scroll_bar(true)
                        .reveal_caret_on_focus(reveal)
                        .on_edit(|s: &mut G| &mut s.focus_caret_edit)
                        .build(k)
                        .key("focus-caret-area"),
                )
                .build(k),
        )
        .child(text(status).key("focus-caret-status").font(k.ty.small).color(k.c.text_muted).wrap())
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

/// TextInput, TextArea and TextField forwarding `on_secondary_press`, `on_input_paste`,
/// `description` and `on_submit` to their inner editors.
pub(super) fn text_input_hooks(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let intercept = !s.is("hooks-pass-paste");
    let status = s
        .pick
        .get("hooks-status")
        .map_or("Right-click any field, paste text or files, or press Return in TextField.", String::as_str);
    let (target, target_idx): (&'static str, usize) = match s.pick.get("hooks-menu-field").map(String::as_str) {
        Some("TextArea") => ("TextArea", 5),
        Some("TextField") => ("TextField", 6),
        _ => ("TextInput", 4),
    };
    let open_menu = |name: &'static str| {
        move |s: &mut G, p: rune::PointerPress, _: &mut Tx| {
            s.menu_at = Some(p);
            s.pick.insert("hooks-menu-field".into(), name.into());
            s.pick.insert("hooks-status".into(), format!("Context menu on {name} at ({:.0}, {:.0})", p.x, p.y));
        }
    };
    let on_paste = |idx: usize, name: &'static str| {
        move |s: &mut G, content: &rune::ClipboardContent, _: &mut Tx| {
            let files = content.files().len();
            if files > 0 {
                s.edits[idx].insert(&format!("[{files} file(s)]"));
                s.pick.insert("hooks-status".into(), format!("{name} intercepted {files} file(s)"));
                return true;
            }
            if !s.is("hooks-pass-paste") {
                let trimmed = content.text().trim();
                s.edits[idx].insert(&format!("[pasted: {trimmed}]"));
                s.pick.insert("hooks-status".into(), format!("{name} intercepted paste ({} chars)", trimmed.len()));
                return true;
            }
            s.pick.insert("hooks-status".into(), format!("{name} passed paste through"));
            false
        }
    };
    let body = grid(16.0, 320.0)
        .child(
            Checkbox::new(intercept)
                .label("Intercept paste in on_input_paste")
                .on_toggle(flip("hooks-pass-paste"))
                .build(k)
                .key("hooks-intercept"),
        )
        .child(
            Field::new()
                .label("TextInput")
                .control(
                    TextInput::new(&s.edits[4])
                        .label("TextInput")
                        .description("Right-click for menu or paste to intercept")
                        .placeholder("Right-click or paste…")
                        .width(None)
                        .on_edit(|s: &mut G| &mut s.edits[4])
                        .on_secondary_press(open_menu("TextInput"))
                        .on_input_paste(on_paste(4, "TextInput"))
                        .build(k)
                        .key("hooks-input"),
                )
                .build(k),
        )
        .child(
            Field::new()
                .label("TextArea")
                .control(
                    TextArea::new(&s.edits[5])
                        .label("TextArea")
                        .description("Multi-line editor with custom menu and paste hook")
                        .placeholder("Right-click anywhere in box or paste…")
                        .width(None)
                        .on_edit(|s: &mut G| &mut s.edits[5])
                        .on_secondary_press(open_menu("TextArea"))
                        .on_input_paste(on_paste(5, "TextArea"))
                        .build(k)
                        .key("hooks-area"),
                )
                .build(k),
        )
        .child(
            TextField::new("TextField", &s.edits[6])
                .placeholder("Type and press Return…")
                .description("Supports description, on_submit, paste and menu")
                .on_edit(|s: &mut G| &mut s.edits[6])
                .on_submit(|s: &mut G, _| {
                    let v = s.edits[6].text.clone();
                    s.pick.insert("hooks-status".into(), format!("TextField submitted: {v:?}"));
                })
                .on_secondary_press(open_menu("TextField"))
                .on_input_paste(on_paste(6, "TextField"))
                .build(k)
                .key("hooks-field"),
        )
        .child(text(status).key("hooks-status").font(k.ty.small).color(k.c.text_muted).wrap());

    context_menu("text-hooks-menu", body, s.menu_at)
        .on_close(|s: &mut G| s.menu_at = None)
        .item(MenuItem::new(format!("Insert sample in {target}")).on_select(move |s: &mut G| {
            s.edits[target_idx].insert("sample");
            s.menu_at = None;
            s.pick.insert("hooks-status".into(), format!("Inserted sample in {target}"));
        }))
        .item(MenuItem::new(format!("Clear {target}")).on_select(move |s: &mut G| {
            s.edits[target_idx].text.clear();
            s.edits[target_idx].set_caret(0);
            s.menu_at = None;
            s.pick.insert("hooks-status".into(), format!("Cleared {target}"));
        }))
        .build(k, layer)
}

// Select

const FRUITS: [&str; 5] = ["Apple", "Banana", "Cherry", "Date", "Elderberry"];

fn fruit_select(s: &G, key: &'static str) -> Select {
    let current = s.pick.get(key).cloned();
    Select::new(key, &FRUITS)
        .label("Fruit")
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
    row().child(fruit_select(s, "select-playground").build(k, layer))
}

pub(super) fn select_grouped(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let cities =
        [("North", "Boston"), ("South", "Miami"), ("South", "Atlanta"), ("West", "Seattle"), ("West", "Denver")];
    let current = s.pick.get("select-grouped").cloned();
    row().child(
        Select::grouped("select-grouped", &cities)
            .label("City")
            .placeholder("Pick a fruit")
            .current(current.as_deref())
            .open(is_open(s, "select-grouped"))
            .on_toggle(toggle("select-grouped"))
            .on_close(close)
            .on_choose(|s: &mut G, v: &str, _: &mut Tx| {
                s.pick.insert("select-grouped".into(), v.to_owned());
                s.open = None;
            })
            .build(k, layer),
    )
}

pub(super) fn select_invalid(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(fruit_select(s, "select-invalid").invalid(true).build(k, layer))
}

pub(super) fn select_disabled(_: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(Select::new("select-disabled", &FRUITS).current(Some("Cherry")).disabled(true).build(k, layer))
}

/// Both triggers in every state, forced, and an open field with its listbox: the inline trigger's
/// rest, hover, open and disabled; the 280 x 28 field's rest, hover, keyboard focus, open,
/// invalid and disabled.
pub(super) fn select_states(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let pick = |key: &'static str, st: State| Select::new(key, &FRUITS).label("Fruit").placeholder("Pick a fruit").current(Some("Banana")).state(st);
    let line = |name: &str, e: El| row().gap(16.0).center_y().child(text(name.to_owned()).font(k.ty.small).color(k.c.text_faint).w(72.0)).child(e);
    let field = |key: &'static str, st: State| pick(key, st).appearance(SelectAppearance::Field);
    col().gap(10.0)
        .child(line("Inline", pick("i1", State::Rest).trigger(k)))
        .child(line("Hover", pick("i2", State::Hover).trigger(k)))
        .child(line("Open", pick("i3", State::Rest).open(true).trigger(k)))
        .child(line("Disabled", pick("i4", State::Rest).disabled(true).trigger(k)))
        .child(line("Field", field("f1", State::Rest).trigger(k)))
        .child(line("Hover", field("f2", State::Hover).trigger(k)))
        .child(line("Focus", field("f3", State::Focus).trigger(k)))
        .child(line("Invalid", field("f4", State::Rest).invalid(true).trigger(k)))
        .child(line("Disabled", field("f5", State::Rest).disabled(true).trigger(k)))
        .child(line("Open", fruit_select(s, "select-states").appearance(SelectAppearance::Field).build(k, layer)))
}

pub(super) fn select_with_field(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Fruit")
            .info()
            .prefix("Optional helper")
            .control(
                fruit_select(s, "select-with-field")
                    .appearance(SelectAppearance::Field)
                    .fill()
                    .menu_width(280.0)
                    .build(k, layer),
            )
            .suffix("After selection")
            .build(k),
    )
}

/// A closed `Select` opted into `step_closed(true)`: Up and Down step through options without
/// opening the popup, while Alt+Down, Space and clicking still open it.
pub(super) fn select_step_closed(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    const THEMES: [&str; 3] = ["System", "Light", "Dark"];
    let current = s.picked("step-closed", "Light");
    let select = Select::new("step-closed", &THEMES)
        .label("Theme")
        .placeholder("Choose theme")
        .appearance(SelectAppearance::Field)
        .width(200.0)
        .current(Some(current))
        .open(is_open(s, "step-closed"))
        .step_closed(true)
        .on_toggle(toggle("step-closed"))
        .on_close(close)
        .on_choose(|s: &mut G, v: &str, _: &mut Tx| {
            s.pick.insert("step-closed".into(), v.to_owned());
            s.open = None;
        })
        .build(k, layer);
    col()
        .gap(10.0)
        .items_start()
        .child(select)
        .child(
            text(format!("Selected: {current} — Up/Down steps while closed; Alt+Down or Space opens."))
                .font(k.ty.small)
                .color(k.c.text_muted),
        )
}
