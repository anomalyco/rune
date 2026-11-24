//! Keybind (`@opencode/ui` data-display/keybind): small keycaps. A single character is
//! upper-cased ("k" draws K) unless [`Keybind::keep_case`] is set; names such as
//! `Shortcut::keycaps`'s "Space" draw as given.

use rune::{El, TextStyle, row, text};

use crate::Kit;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KeybindVariant {
    #[default]
    Neutral,
    Ghost,
}

pub struct Keybind {
    keys: Vec<String>,
    variant: KeybindVariant,
    keep_case: bool,
}

impl Keybind {
    pub fn new<S: AsRef<str>>(keys: &[S]) -> Keybind {
        Keybind {
            keys: keys.iter().map(|k| k.as_ref().to_owned()).collect(),
            variant: KeybindVariant::Neutral,
            keep_case: false,
        }
    }
    pub fn variant(mut self, v: KeybindVariant) -> Self {
        self.variant = v;
        self
    }
    /// Keeps each keycap's authored case instead of upper-casing single characters.
    ///
    /// By default, a single character such as `"g"` draws as `"G"`. Call `keep_case` when
    /// letter case distinguishes bindings (for example Vim-style `g` vs `G`).
    ///
    /// ```
    /// use rune_kit::{Keybind, Kit};
    ///
    /// let k = Kit::neutral(true);
    /// let lower = Keybind::new(&["g"]).keep_case().build(&k);
    /// let upper = Keybind::new(&["Shift", "G"]).keep_case().build(&k);
    /// # let _ = (lower, upper);
    /// ```
    pub fn keep_case(mut self) -> Self {
        self.keep_case = true;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let (fill, ink) = match self.variant {
            KeybindVariant::Neutral => (c.layer_03, c.text_muted),
            KeybindVariant::Ghost => (rune::Color::TRANSPARENT, c.text_faint),
        };
        let font = TextStyle { line_height: 11.0, ..k.ty.small_medium };
        let label = self.keys.join(" ");
        let keep_case = self.keep_case;
        row().role("group").label(label).gap(2.0).center_y().children(self.keys.into_iter().enumerate().map(
            |(i, key)| {
                let cap = if keep_case { key } else { keycap(key) };
                row()
                    .key(format!("key{i}"))
                    .min_w(14.0)
                    .h(14.0)
                    .px(4.0)
                    .center()
                    .radius(2.0)
                    .fill(fill)
                    .child(text(cap).font(font).color(ink))
                    .a11y_hidden()
            },
        ))
    }
}

fn keycap(key: String) -> String {
    let upper = key.to_uppercase();
    // Only when it maps back: Turkish "ı" would draw as "I", another key.
    if key.chars().count() == 1 && upper.to_lowercase() == key { upper } else { key }
}
