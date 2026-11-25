//! Rune's own combobox scenes; there is no Storybook story to match.

use rune::{El, Key, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, Combobox, ComboboxOption, ControlSize, Kit, Placement, TextInput, Tooltip};

use crate::G;

pub(super) fn searchable(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    models(s, "model", &s.combo, |s: &mut G| &mut s.combo, false).build(k, layer)
}

/// The same list, crossfading each keystroke's rows and springing the popover's height.
pub(super) fn animated_filter(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    models(s, "model-animated", &s.combo_animated, |s: &mut G| &mut s.combo_animated, true).build(k, layer)
}

/// Localised labels: outer label, placeholder, search placeholder, search accessible name and empty message.
pub(super) fn localized(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let options = [
        ComboboxOption::new("rapide", "Rapide"),
        ComboboxOption::new("precis", "Précis"),
        ComboboxOption::new("equilibre", "Équilibré"),
    ];
    Combobox::new("modele", &s.combo_localized, &options, |s: &mut G| &mut s.combo_localized)
        .label("Modèle")
        .placeholder("Choisir un modèle")
        .search_placeholder("Rechercher des modèles…")
        .search_label("Rechercher des modèles")
        .empty_message("Aucun modèle")
        .current(s.pick.get("modele").map(String::as_str))
        .on_choose(|s, id, _| { s.pick.insert("modele".into(), id.into()); })
        .build(k, layer)
}

/// Ancestor `El::on_key_preview` closing a tooltip anchored on a combobox text input on the first
/// Escape before the focused input's own `on_key` closes the open option list on the second Escape.
pub(super) fn key_preview(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    const OPTIONS: [&str; 4] = ["Claude Sonnet", "GPT-5", "Gemini Pro", "Llama 4"];
    let tip_open = s.is("kp-tip");
    let list_open = s.is("kp-list");
    let query = s.key_preview_edit.text.trim().to_lowercase();
    let matches: Vec<&str> = OPTIONS
        .into_iter()
        .filter(|o| query.is_empty() || o.to_lowercase().contains(&query))
        .collect();
    let last_event = s.picked("kp-log", "Focus input and press Escape twice");

    let field = TextInput::new(&s.key_preview_edit)
        .width(Some(280.0))
        .placeholder("Filter models (press Escape)…")
        .on_edit(|s: &mut G| &mut s.key_preview_edit)
        .on_change(|s: &mut G, _| {
            s.on.insert("kp-list".into());
        })
        .on_key(|s: &mut G, ev, tx| {
            if ev.key == Key::Escape && !tx.key_repeat() && s.is("kp-list") {
                s.on.remove("kp-list");
                s.pick.insert("kp-log".into(), "2nd Escape: focused input closed option list".into());
                return true;
            }
            false
        })
        .build(k)
        .key("kp-input");

    let anchored = Tooltip::new("kp-tip")
        .trigger(field)
        .placement(Placement::Top)
        .open(tip_open)
        .text(k, "1st Escape closes this tooltip before the list")
        .build(k, layer);

    let mut wrap = col()
        .key("kp-wrap")
        .w(320.0)
        .gap(8.0)
        .padding(12.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .on_key_preview(|s: &mut G, ev, tx| {
            if ev.key == Key::Escape && !tx.key_repeat() && s.is("kp-tip") {
                s.on.remove("kp-tip");
                s.pick.insert("kp-log".into(), "1st Escape: ancestor on_key_preview closed tooltip".into());
                return true;
            }
            false
        })
        .child(
            row()
                .w_full()
                .center_y()
                .justify_between()
                .child(text("El::on_key_preview").font(k.ty.ui_medium).color(k.c.text_strong))
                .child(
                    Button::new("Reset")
                        .variant(ButtonVariant::Ghost)
                        .control_size(ControlSize::Small)
                        .on_press_tx(|s: &mut G, tx: &mut Tx| {
                            s.on.insert("kp-tip".into());
                            s.on.insert("kp-list".into());
                            s.pick.remove("kp-log");
                            tx.focus("kp-input", true);
                        })
                        .build(k)
                        .key("kp-reset"),
                ),
        )
        .child(anchored);

    if list_open {
        let mut list = col()
            .key("kp-list-box")
            .w(280.0)
            .padding(4.0)
            .gap(2.0)
            .radius(k.radius.md)
            .fill(k.c.surface_raised)
            .border(1.0, k.c.border_base);
        if matches.is_empty() {
            list = list.child(text("No matching models").font(k.ty.small).color(k.c.text_muted).padding(6.0));
        } else {
            for opt in matches {
                list = list.child(
                    row()
                        .key(format!("kp-opt-{opt}"))
                        .w_full()
                        .h(28.0)
                        .px(8.0)
                        .center_y()
                        .radius(k.radius.sm)
                        .hover(k.c.hover)
                        .on_press(move |s: &mut G| {
                            s.key_preview_edit = rune::TextEdit::new(opt);
                            s.key_preview_edit.focused = true;
                            s.on.remove("kp-list");
                        })
                        .child(text(opt).font(k.ty.ui_compact).color(k.c.text_base)),
                );
            }
        }
        wrap = wrap.child(list);
    }

    wrap = wrap
        .child(
            text(format!(
                "Tooltip: {} · List: {}",
                if tip_open { "Open" } else { "Closed" },
                if list_open { "Open" } else { "Closed" },
            ))
            .font(k.ty.small_medium)
            .color(k.c.text_base),
        )
        .child(text(last_event).font(k.ty.small).color(k.c.text_muted).wrap());

    col().padding(48.0).items_start().child(wrap)
}

fn models(
    s: &G,
    key: &'static str,
    state: &rune_kit::ComboboxState,
    access: impl for<'a> Fn(&'a mut G) -> &'a mut rune_kit::ComboboxState + 'static,
    animate: bool,
) -> Combobox<G> {
    let options: Vec<_> = (0..200).map(|i| ComboboxOption::new(format!("model-{i}"), format!("Model {i:03}"))).collect();
    Combobox::new(key, state, &options, access)
        .label("Model").placeholder("Choose model").current(s.pick.get(key).map(String::as_str))
        .animate_filtering(animate)
        .on_choose(move |s, id, _| { s.pick.insert(key.into(), id.into()); })
}
