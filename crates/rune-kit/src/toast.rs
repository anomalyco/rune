//! Toast and ToastRegion (`@opencode/ui` feedback/toast).

use rune::{Edge, El, Icon, TextStyle, Transition, Tx, col, el, icon, row, text};

use crate::{Acts, Button, ButtonSize, ButtonVariant, Kit, press_over, shadows};

pub struct Toast {
    title: String,
    description: Option<String>,
    icon: Option<El>,
    actions: Vec<El>,
    close: Option<Acts>,
    key: String,
}

impl Toast {
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Toast {
        Toast { key: key.into(), title: title.into(), description: None, icon: None, actions: Vec::new(), close: None }
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    /// A 16 point icon in the theme's base icon colour.
    pub fn icon(mut self, k: &Kit, i: &'static Icon) -> Self {
        self.icon = Some(icon(i, 16.0).color(k.c.icon_base));
        self
    }
    /// Any 16 point leading element (coloured icons, a loader).
    pub fn lead(mut self, e: El) -> Self {
        self.icon = Some(e);
        self
    }
    /// A primary (neutral) or secondary (ghost) small action button.
    pub fn action<S: 'static>(
        mut self,
        k: &Kit,
        label: &str,
        primary: bool,
        f: impl Fn(&mut S, &mut Tx) + 'static,
    ) -> Self {
        let v = if primary { ButtonVariant::Neutral } else { ButtonVariant::Ghost };
        self.actions.push(Button::new(label).variant(v).size(ButtonSize::Small).on_press_tx(f).build(k));
        self
    }
    /// Shows the close button; `f` dismisses.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        let mut a = Acts::default();
        a.press(f);
        self.close = Some(a);
        self
    }
    /// Shows the close button without a handler (galleries).
    pub fn closable(mut self) -> Self {
        self.close.get_or_insert_with(Acts::default);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let mut content = col().key("content").grow(1.0).shrink().gap(4.0).child(
            text(self.title.clone())
                .key("title")
                .font(TextStyle { line_height: 20.0, ..k.ty.ui_medium })
                .color(c.text_base)
                .truncate(),
        );
        if let Some(d) = self.description {
            content = content.child(text(d).key("description").font(k.ty.ui).color(c.text_muted).wrap().w_full());
        }
        let mut top = row().key("top").w_full().gap(12.0).items_start();
        if let Some(i) = self.icon {
            top = top.child(el().key("icon").size(16.0, 20.0).center().child(i));
        }
        top = top.child(content);
        if let Some(a) = self.close {
            let b = row()
                .key("close")
                .role("button")
                .label("Close")
                .size(20.0, 20.0)
                .center()
                .radius(4.0)
                .hover(c.hover)
                .press(press_over(c.hover, c.pressed))
                .child(icon(k.glyph.close, 16.0).color(c.icon_muted));
            top = top.child(a.apply(b, true));
        }
        let mut root = col()
            .key(self.key)
            .role("status")
            .label(self.title)
            .w(320.0)
            .padding(12.0)
            .gap(12.0)
            .radius(8.0)
            .fill(c.layer_01);
        root = shadows(k, root, &k.elevation.floating).child(top);
        if !self.actions.is_empty() {
            let indent = 16.0 + 12.0;
            root = root.child(row().key("actions").pl(indent).gap(8.0).flex_wrap().children(self.actions));
        }
        root.transition(Transition::move_edge(Edge::Bottom)).motion(rune::Spring::new(0.28, 0.0))
    }
}

/// The bottom-right stack toasts live in (32 points from the right, 48 from the bottom,
/// 12 apart). Mount it with `crate::root` as an overlay.
pub struct ToastRegion {
    toasts: Vec<El>,
}

impl ToastRegion {
    pub fn new() -> ToastRegion {
        ToastRegion { toasts: Vec::new() }
    }
    pub fn toast(mut self, t: El) -> Self {
        self.toasts.push(t);
        self
    }
    pub fn build(self, _k: &Kit) -> El {
        col()
            .key("toast-region")
            .role("region")
            .label("Notifications")
            .pin(None, None, Some(32.0), Some(48.0))
            .gap(12.0)
            .children(self.toasts)
    }
}

impl Default for ToastRegion {
    fn default() -> Self {
        Self::new()
    }
}
