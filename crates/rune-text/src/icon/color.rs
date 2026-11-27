//! Icons that paint their own colours (`fill="#…"`, gradients), such as file-type badges. They
//! rasterize in colour with resvg and draw as images; a monochrome icon stays an alpha mask in
//! the glyph atlas, tinted by its element's colour.
//!
//! Classification is cached once per swapped `&'static Icon` address in an FxHashMap so static
//! icons pay no extra lock or second hash lookup on the hot paint path. Multi-colour icons with
//! SMIL animation elements deliberately render static (no UI-thread `usvg` animation).

use std::collections::HashMap;
use std::hash::BuildHasherDefault;

use rune_core::color::Color;
use rune_image::pixels::Pixels;

use super::anim;
use crate::{Icon, TextSystem};

/// CPU bytes of colour icon images kept before images unused last frame are dropped.
const BUDGET: usize = 4 * 1024 * 1024;

type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<rune_motion::Fx>>;

#[derive(Default)]
pub(crate) struct ColorIcons {
    /// What each swapped icon, by address, paints with.
    kinds: FxMap<usize, Kind>,
    /// Each colour icon's image by address, device-pixel side and `currentColor` (`#rrggbb`, or
    /// empty for an icon without it); None when it can't rasterize.
    images: HashMap<(usize, u32, String), Entry>,
    bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mono,
    AnimatedMono,
    Color,
    /// Its own colours, and `currentColor` parts that take the element's colour.
    ColorAndCurrent,
}

/// Classification of an icon on the hot paint path (single cached lookup).
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconKind {
    /// Static monochrome icon drawn from the glyph atlas.
    Mono,
    /// Monochrome SMIL animated icon drawn from double-buffered glyph atlas tiles.
    AnimatedMono,
    /// Static multi-colour icon drawn via `color_icon`.
    Color,
}

struct Entry {
    pixels: Option<Pixels>,
    used: bool,
}

impl ColorIcons {
    pub(crate) fn begin_frame(&mut self) {
        if self.bytes > BUDGET {
            self.images.retain(|_, e| e.used);
            self.bytes = self.images.values().filter_map(|e| e.pixels.as_ref()).map(bytes_of).sum();
        }
        self.images.values_mut().for_each(|e| e.used = false);
    }
}

impl TextSystem {
    /// Classifies `icon` (after applying any active icon swap) in a single cached FxHashMap
    /// lookup. On first encounter of a monochrome animated icon, compiles and caches its bounded
    /// AST. Multi-colour icons stay static even if they contain SMIL tags (logged once in debug
    /// builds).
    #[doc(hidden)]
    pub fn icon_kind(&mut self, icon: &'static Icon) -> IconKind {
        let icon = super::swapped(icon);
        let at = icon as *const Icon as usize;
        let k = if let Some(&k) = self.color_icons.kinds.get(&at) {
            k
        } else {
            let classified = classify(icon, &mut self.animated_icons);
            self.color_icons.kinds.insert(at, classified);
            classified
        };
        match k {
            Kind::Mono => IconKind::Mono,
            Kind::AnimatedMono => IconKind::AnimatedMono,
            Kind::Color | Kind::ColorAndCurrent => IconKind::Color,
        }
    }

    /// Whether `icon` paints its own colours, and so draws as an image ([`Self::color_icon`]).
    #[doc(hidden)]
    pub fn is_color_icon(&mut self, icon: &'static Icon) -> bool {
        self.icon_kind(icon) == IconKind::Color
    }

    /// The colour image of `icon` at `px` device pixels square when its body names its own
    /// colours, its `currentColor` parts in `color`; None for a monochrome icon, which draws
    /// from the alpha atlas ([`Self::icon_quad`]). Without Rune's `svg` feature every icon draws
    /// monochrome.
    ///
    /// With `one` (`El::icon_one_color`) the image is the icon's silhouette in `color`: its own
    /// colours drawn as their coverage only, as an SVG mask filled with `currentColor` draws it.
    pub fn color_icon(&mut self, icon: &'static Icon, px: u32, color: Color, one: bool) -> Option<&Pixels> {
        let icon = super::swapped(icon);
        let at = icon as *const Icon as usize;
        let k = if let Some(&k) = self.color_icons.kinds.get(&at) {
            k
        } else {
            let classified = classify(icon, &mut self.animated_icons);
            self.color_icons.kinds.insert(at, classified);
            classified
        };
        let ColorIcons { images, bytes, .. } = &mut self.color_icons;
        let hex = || color.hex()[..7].to_owned();
        let current = match k {
            Kind::Mono | Kind::AnimatedMono => return None,
            _ if one => format!("one{}", hex()),
            Kind::Color => String::new(),
            Kind::ColorAndCurrent => hex(),
        };
        let entry = images.entry((at, px, current)).or_insert_with_key(|(_, _, current)| {
            let pixels = match current.strip_prefix("one") {
                Some(hex) => rasterize(icon, px, hex, Some(color)),
                None => rasterize(icon, px, current, None),
            };
            *bytes += pixels.as_ref().map_or(0, bytes_of);
            Entry { pixels, used: false }
        });
        entry.used = true;
        entry.pixels.as_ref()
    }
}

fn classify(icon: &'static Icon, animated: &mut anim::AnimatedIcons) -> Kind {
    let b = icon.body;
    if ["fill=\"#", "stroke=\"#", "stop-color=\"#"].iter().any(|p| b.contains(p)) {
        #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
        if anim::may_be_animated(b) {
            eprintln!(
                "rune: multi-colour icon {:?} contains SVG animation elements; colour icon animation is ignored and the icon renders static",
                icon.name
            );
        }
        return if b.contains("currentColor") { Kind::ColorAndCurrent } else { Kind::Color };
    }
    if anim::may_be_animated(b)
        && let anim::CompileOutcome::Animated(plan) = anim::compile(b, icon.view)
        && animated.insert_plan(icon as *const Icon as usize, plan)
    {
        return Kind::AnimatedMono;
    }
    Kind::Mono
}

fn bytes_of(p: &Pixels) -> usize {
    let (w, h) = p.size();
    w as usize * h as usize * 4
}

#[cfg(feature = "svg")]
fn rasterize(icon: &Icon, px: u32, current: &str, one: Option<Color>) -> Option<Pixels> {
    if px == 0 {
        return None;
    }
    let vb = icon.view_box()?;
    let body = if current.is_empty() { icon.body.to_owned() } else { icon.body.replace("currentColor", current) };
    let doc = super::view_box::svg_document(&body, vb, px);
    let (w, h, mut rgba) = rune_image::svg_rgba(doc.as_bytes(), px).ok()?;
    // One colour: the drawing's coverage in it; its alpha comes from the instance.
    if let Some(c) = one {
        let rgb = [c.r, c.g, c.b].map(|v| (rune_core::color::srgb_encode(v) * 255.0).round() as u8);
        rgba.chunks_exact_mut(4).for_each(|p| p[..3].copy_from_slice(&rgb));
    }
    let pixels = Pixels::new();
    pixels.set(w, h, rgba);
    Some(pixels)
}

#[cfg(not(feature = "svg"))]
fn rasterize(_: &Icon, _: u32, _: &str, _: Option<Color>) -> Option<Pixels> {
    None
}
