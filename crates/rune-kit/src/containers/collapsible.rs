//! Collapsible (`@opencode/ui` components/collapsible): a 32 point trigger row with a
//! disclosure arrow that shows on hover or when open, and content below.
//!
//! The arrow can stay visible ([`Collapsible::arrow_always`]), sit at the row's end
//! ([`Collapsible::arrow_end`]) and take its own size ([`Collapsible::arrow_size`]). Controls
//! such as a reset button or a switch go after the trigger in the same header row
//! ([`Collapsible::trailing`]), outside the trigger button so they keep their own presses.

use rune::{El, Tx, col, el, icon, row, spacer, text};

use crate::{Acts, Kit, State};

pub struct Collapsible {
    title: String,
    open: bool,
    ghost: bool,
    arrow_always: bool,
    arrow_end: bool,
    arrow_size: f32,
    trailing: Vec<El>,
    content: Vec<El>,
    state: State,
    acts: Acts,
}

impl Collapsible {
    pub fn new(title: impl Into<String>, open: bool) -> Collapsible {
        Collapsible {
            title: title.into(),
            open,
            ghost: false,
            arrow_always: false,
            arrow_end: false,
            arrow_size: 14.0,
            trailing: Vec::new(),
            content: Vec::new(),
            state: State::Rest,
            acts: Acts::default(),
        }
    }
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }
    /// Draws the arrow while closed and not hovered too, so a closed section reads as one that
    /// opens. By default the arrow shows only when open or under the pointer.
    pub fn arrow_always(mut self) -> Self {
        self.arrow_always = true;
        self
    }
    /// Puts the arrow at the end of the trigger row (before any [`Collapsible::trailing`]
    /// content) instead of right after the title.
    pub fn arrow_end(mut self) -> Self {
        self.arrow_end = true;
        self
    }
    /// The arrow icon's size in points; else `14.0`. Its box stays 24 points, or the icon's
    /// size when larger.
    pub fn arrow_size(mut self, size: f32) -> Self {
        self.arrow_size = size.max(0.0);
        self
    }
    /// Adds content at the end of the header row, such as a reset button or a switch. It sits
    /// beside the trigger, not inside it, so pressing it does not toggle the section.
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing.push(e);
        self
    }
    pub fn child(mut self, c: El) -> Self {
        self.content.push(c);
        self
    }
    pub fn state(mut self, s: State) -> Self {
        self.state = s;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        if d {
            self.state = State::Disabled;
        }
        self
    }
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_toggle_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let show = self.arrow_always || self.open || self.state == State::Hover;
        let arrow_box = self.arrow_size.max(24.0);
        let glyph = if self.open { k.glyph.triangle_down } else { k.glyph.triangle_right };
        let arrow = el()
            .key("arrow")
            .size(arrow_box, arrow_box)
            .center()
            .opacity(if show { 1.0 } else { 0.0 })
            .child(icon(glyph, self.arrow_size).color(c.icon_weaker));
        let header_height = k.disclosure_header_height();
        let enabled = self.state != State::Disabled;
        let title = text(self.title.clone()).key("title").font(k.ty.text_small_medium).color(c.std_text);
        let mut inner = row().gap(8.0).center_y().child(title);
        inner = if self.arrow_end { inner.w_full().child(spacer()).child(arrow) } else { inner.child(arrow) };
        let has_trailing = !self.trailing.is_empty();
        let trigger = row()
            .key("trigger")
            .role("button")
            .label(self.title.clone())
            .expanded(self.open)
            .focusable(enabled)
            .disabled(!enabled);
        let trigger = if has_trailing { trigger.grow(1.0).min_w(0.0) } else { trigger.w_full() };
        let trigger = k.focus_ring(
            trigger
                .h(header_height)
                .radius(k.radius_sm())
                .center_y()
                .child(inner.a11y_hidden()),
            2.0,
            2.0,
        );
        let trigger = self.acts.apply(trigger, enabled);
        let header = if has_trailing {
            let trailing = row().key("trailing").gap(8.0).center_y().children(self.trailing);
            row().key("header").w_full().h(header_height).gap(8.0).center_y().child(trigger).child(trailing)
        } else {
            trigger
        };
        let mut root = col().role("group").w_full().radius(k.radius_md()).child(header);
        if self.open {
            root = root.child(col().key("content").role("region").label(self.title).w_full().children(self.content));
        }
        root
    }
}
