//! Accordion (`@opencode/ui` data-display/accordion): bordered stacked triggers whose content
//! opens below, sharing borders, with rounded outer corners.

use rune::{El, Tx, col, row, text};

use crate::{Acts, Kit, State};

pub struct AccordionItem {
    title: String,
    expanded: bool,
    content: Vec<El>,
    state: State,
    acts: Acts,
}

impl AccordionItem {
    pub fn new(title: impl Into<String>) -> AccordionItem {
        AccordionItem {
            title: title.into(),
            expanded: false,
            content: Vec::new(),
            state: State::Rest,
            acts: Acts::default(),
        }
    }
    pub fn expanded(mut self, e: bool) -> Self {
        self.expanded = e;
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
}

pub struct Accordion {
    items: Vec<AccordionItem>,
    width: Option<f32>,
}

impl Accordion {
    pub fn new() -> Accordion {
        Accordion { items: Vec::new(), width: None }
    }
    pub fn item(mut self, i: AccordionItem) -> Self {
        self.items.push(i);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let r = k.radius.lg;
        let n = self.items.len();
        let mut root = col().role("group").items_stretch();
        root = match self.width {
            Some(w) => root.w(w),
            None => root.w_full(),
        };
        for (i, item) in self.items.into_iter().enumerate() {
            let (first, last) = (i == 0, i + 1 == n);
            let bottom = if last && !item.expanded { r } else { 0.0 };
            let top = if first { r } else { 0.0 };
            let mut trigger = row()
                .key("trigger")
                .role("button")
                .label(item.title.clone())
                .h(32.0)
                .px(12.0)
                .py(8.0)
                .center_y()
                .justify_between()
                .corners([top, top, bottom, bottom])
                .fill(if item.state == State::Hover { c.hover } else { c.bg_base })
                .border(1.0, c.border_base)
                .token_motion(k.motion.control)
                .child(text(item.title).key("title").font(k.ty.text_small).color(c.text_base));
            if item.state == State::Rest {
                trigger = trigger.hover(c.hover);
            }
            let enabled = item.state != State::Disabled;
            let trigger = item.acts.apply(trigger, enabled);
            let mut e = col().key(format!("item{i}")).items_stretch().child(trigger);
            if i > 0 {
                e = e.mt(-1.0);
            }
            if item.expanded {
                let b = if last { r } else { 0.0 };
                // The content's own top border would double the trigger's (both translucent):
                // draw it one point up and clip that row away, as CSS's `border-top: 0` does.
                e = e.child(
                    col().key("content-clip").clip_soft(0.0).child(
                        col()
                            .key("content")
                            .corners([0.0, 0.0, b, b])
                            .fill(c.layer_01)
                            .border(1.0, c.border_base)
                            .mt(-1.0)
                            .pt(1.0)
                            .children(item.content),
                    ),
                );
            }
            root = root.child(e);
        }
        root
    }
}

impl Default for Accordion {
    fn default() -> Self {
        Self::new()
    }
}
