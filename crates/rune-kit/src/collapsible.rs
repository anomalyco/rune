//! Collapsible (`@opencode/ui` components/collapsible): a 32 point trigger row with a
//! disclosure arrow that shows on hover or when open, and content below.

use rune::{El, Tx, col, el, icon, row, text};

use crate::{Acts, Kit, State};

pub struct Collapsible {
    title: String,
    open: bool,
    ghost: bool,
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
            content: Vec::new(),
            state: State::Rest,
            acts: Acts::default(),
        }
    }
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
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
        let show = self.open || self.state == State::Hover;
        let glyph = if self.open { k.glyph.triangle_down } else { k.glyph.triangle_right };
        let arrow = el()
            .key("arrow")
            .size(24.0, 24.0)
            .center()
            .opacity(if show { 1.0 } else { 0.0 })
            .child(icon(glyph, 14.0).color(c.icon_weaker));
        let enabled = self.state != State::Disabled;
        let trigger = row()
            .key("trigger")
            .role("button")
            .label(self.title.clone())
            .expanded(self.open)
            .focusable(enabled)
            .focus_ring(2.0, 0.0, c.border_focus)
            .disabled(!enabled)
            .w_full()
            .h(32.0)
            .center_y()
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(text(self.title.clone()).key("title").font(k.ty.text_small_medium).color(c.std_text))
                    .child(arrow)
                    .a11y_hidden(),
            );
        let trigger = self.acts.apply(trigger, enabled);
        let mut root = col().role("group").w_full().radius(k.radius.md).child(trigger);
        if self.open {
            root = root.child(col().key("content").role("region").label(self.title).w_full().children(self.content));
        }
        root
    }
}
