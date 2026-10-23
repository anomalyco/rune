//! The rune-kit gallery: one scene per `@opencode/ui` Storybook story, in the story's id, so
//! `scripts/kit-diff.py` can render both and diff them. `--kit neutral` renders the same
//! components on Rune's core neutral theme instead of OpenCode's; `--kit palette` on
//! `Kit::from_palette` with a near-black, off-white and red seed (Deadbolt's palette);
//! `--kit palette black` (dark) and `--kit palette warm` (light) use Deadbolt's pure-black and
//! paper seeds with ink focus rings. `--reduced-motion` renders in the runtime's reduced-motion
//! mode.
//!
//! ```sh
//! kit-gallery --list-scenes
//! kit-gallery --scene ui-button--variants --snap out.png --theme dark --size 720x300
//! ```

mod stories;

#[cfg(test)]
mod layer_timing;

use std::collections::{HashMap, HashSet};

use rune::{AppSpec, Cx, El, TextEdit, col};
use rune_kit::Kit;

pub struct G {
    pub story: &'static str,
    /// Toggled things by key (switches, checkboxes, expanded items).
    pub on: HashSet<String>,
    /// Picked values by group key (radio groups, selects, segmented controls, tabs).
    pub pick: HashMap<String, String>,
    /// The open overlay, by key.
    pub open: Option<String>,
    pub edits: [TextEdit; 8],
    pub combo: rune_kit::ComboboxState,
    pub neutral: bool,
    pub palette: bool,
    /// `--reduced-motion`: the runtime's reduced-motion mode.
    pub reduced: bool,
}

impl G {
    pub fn is(&self, key: &str) -> bool {
        self.on.contains(key)
    }
    pub fn flip(&mut self, key: &str) {
        if !self.on.remove(key) {
            self.on.insert(key.to_owned());
        }
    }
    pub fn picked<'a>(&'a self, group: &str, default: &'a str) -> &'a str {
        self.pick.get(group).map_or(default, String::as_str)
    }
    pub fn toggle_open(&mut self, key: &str) {
        self.open = if self.open.as_deref() == Some(key) { None } else { Some(key.to_owned()) };
    }
}

fn init() -> G {
    G {
        story: "index",
        on: HashSet::new(),
        pick: HashMap::new(),
        open: None,
        edits: Default::default(),
        combo: Default::default(),
        neutral: rune::extra_args().iter().any(|a| a == "neutral"),
        palette: rune::extra_args().iter().any(|a| a == "palette"),
        reduced: rune::extra_args().iter().any(|a| a == "--reduced-motion"),
    }
}

pub fn kit(s: &G, dark: bool) -> &'static Kit {
    if s.palette {
        static D: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        static L: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        let hex = |c: &str| rune::color::hex(c).unwrap();
        let seed = |bg: &str, fg: &str, accent: &str| rune_kit::PaletteSeed { bg: hex(bg), fg: hex(fg), accent: hex(accent) };
        // `--kit palette black|warm`: Deadbolt's pure-black and paper seeds with ink focus.
        let variant = |name: &str| rune::extra_args().iter().any(|a| a == name);
        let ink = |seed| Kit::from_palette_focus(seed, rune_kit::PaletteFocus::Ink);
        return if dark {
            D.get_or_init(|| match variant("black") {
                true => ink(seed("#000000", "#ffffff", "#ff2a2a")),
                false => Kit::from_palette(seed("#0a0a0a", "#ededed", "#e5484d")),
            })
        } else {
            L.get_or_init(|| match variant("warm") {
                true => ink(seed("#e8e4dc", "#000000", "#d81414")),
                false => Kit::from_palette(seed("#ffffff", "#171717", "#e5484d")),
            })
        };
    }
    if s.neutral {
        static D: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        static L: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        return if dark { D.get_or_init(|| Kit::neutral(true)) } else { L.get_or_init(|| Kit::neutral(false)) };
    }
    rune_theme_opencode::kit(dark)
}

fn view(s: &G, cx: &mut Cx<G>) -> El {
    if s.reduced {
        cx.reduced_motion(true);
    }
    let k = kit(s, cx.system_dark());
    let mut overlays = Vec::new();
    let body = stories::render(s, k, &mut overlays);
    // Storybook's decorator: the page colour and 24 px of padding around the story.
    rune_kit::root(col().key("frame").fill(k.c.page).padding(24.0).child(body), overlays)
}

fn main() {
    rune::run(AppSpec {
        scenes: stories::SCENES,
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        ..AppSpec::new("rune-kit gallery", (720.0, 480.0), init, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every story builds on both appearances of both themes (no GPU needed).
    #[test]
    fn every_story_builds() {
        for (neutral, palette) in [(false, false), (true, false), (false, true)] {
            for dark in [false, true] {
                let mut s = init();
                s.neutral = neutral;
                s.palette = palette;
                for (id, setup) in stories::SCENES {
                    setup(&mut s);
                    let k = kit(&s, dark);
                    let mut layer = Vec::new();
                    let _ = stories::render(&s, k, &mut layer);
                    assert_eq!(s.story, *id);
                }
            }
        }
    }
}
