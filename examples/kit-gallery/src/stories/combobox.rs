//! Rune's own combobox scenes; there is no Storybook story to match.

use rune::El;
use rune_kit::{Combobox, ComboboxOption, Kit};

use crate::G;

pub(super) fn searchable(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    models(s, false).build(k, layer)
}

/// The same list, crossfading each keystroke's rows and springing the popover's height.
pub(super) fn animated_filter(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    models(s, true).build(k, layer)
}

fn models(s: &G, animate: bool) -> Combobox<G> {
    let options: Vec<_> = (0..200).map(|i| ComboboxOption::new(format!("model-{i}"), format!("Model {i:03}"))).collect();
    Combobox::new("model", &s.combo, &options, |s: &mut G| &mut s.combo)
        .label("Model").placeholder("Choose model").current(s.pick.get("model").map(String::as_str))
        .animate_filtering(animate)
        .on_choose(|s, id, _| { s.pick.insert("model".into(), id.into()); })
}
