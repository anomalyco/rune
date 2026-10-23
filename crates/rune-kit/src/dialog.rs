//! Modal surface; focus, dismissal and background inertness belong to Rune.
use rune::{El, TextStyle, col, el, row, text};
use crate::{ButtonSize, IconButton, IconButtonVariant, Kit, shadows};

pub struct Dialog {
    key: String,
    title: String,
    description: Option<String>,
    body: Vec<El>,
    open: bool,
    outside: bool,
    width: f32,
    close: Option<Box<dyn FnOnce(El) -> El>>,
    button: Option<Box<dyn FnOnce(El) -> El>>,
}
impl Dialog {
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self { key: key.into(), title: title.into(), description: None, body: Vec::new(), open: false,
            outside: true, width: 480.0, close: None, button: None }
    }
    pub fn open(mut self, open: bool) -> Self { self.open = open; self }
    pub fn description(mut self, description: impl Into<String>) -> Self { self.description = Some(description.into()); self }
    pub fn child(mut self, body: El) -> Self { self.body.push(body); self }
    pub fn width(mut self, width: f32) -> Self { self.width = width; self }
    pub fn click_outside(mut self, close: bool) -> Self { self.outside = close; self }
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + Clone + 'static) -> Self {
        let button = f.clone();
        self.button = Some(Box::new(move |e| e.on_press(button)));
        // The policy is applied at build time, regardless of builder call order.
        self.close = Some(Box::new(move |e| e.modal(true, f)));
        self
    }
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) {
        if !self.open { return; }
        let Some(close) = self.close else { return; };
        let mut dismiss = IconButton::new(k.glyph.close).variant(IconButtonVariant::Ghost)
            .size(ButtonSize::Normal).label("Close").build(k).key("close");
        if let Some(button) = self.button { dismiss = button(dismiss); }
        dismiss = dismiss.size(20.0, 20.0).mt(-2.0).mr(-2.0);
        let mut title = col().grow(1.0).gap(4.0).child(text(self.title.clone())
            .font(if self.description.is_some() { TextStyle { size: 15.0, tracking: -0.13, ..k.ty.ui_medium }.lh(20.0) }
                else { k.ty.ui_medium.lh(16.0) }).color(k.c.text_base));
        if let Some(description) = self.description {
            title = title.child(text(description).font(k.ty.ui.lh(20.0)).color(k.c.text_muted));
        }
        let panel = shadows(k, col().key(format!("{}-dialog", self.key)).role("dialog").label(self.title.clone())
            .w_full().max_w(self.width).h(368.0).radius(6.0).fill(k.c.layer_01)
            .child(row().key("header").w_full().padding(16.0).items_start().child(title).child(dismiss))
            .child(col().key("body").w_full().grow(1.0).children(self.body))
            .motion(k.motion.overlay), &k.elevation.overlay);
        let panel = close(panel).modal_click_outside(self.outside);
        layer.push(el().key(format!("{}-scrim", self.key)).absolute(0.0, 0.0).w_full().h_full()
            .fill(k.c.scrim).motion(k.motion.overlay));
        layer.push(el().key(format!("{}-layer", self.key)).absolute(0.0, 0.0).w_full().h_full().padding(16.0).center().child(panel));
    }
}
