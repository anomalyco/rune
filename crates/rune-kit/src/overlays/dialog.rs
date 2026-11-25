//! Modal surface; focus, dismissal and background inertness belong to Rune. Whether it's open is
//! the app's; its focus lifecycle is Rune's modal stack (`runtime/input/modal_focus.rs`), as
//! Desktop's dialog stack has it (`ui/src/context/dialog-machine.ts`): the top dialog holds the
//! trap, opens on its content and hands focus back to where it was before it opened.
use rune::{El, TextStyle, col, el, row, text};
use super::overlay::scrim_key;
use crate::{ControlSize, IconButton, IconButtonVariant, Kit};

pub struct Dialog {
    key: String,
    title: String,
    description: Option<String>,
    body: Vec<El>,
    open: bool,
    outside: bool,
    width: f32,
    fit: bool,
    close_label: Option<String>,
    close_disabled: bool,
    close: Option<Box<dyn FnOnce(El) -> El>>,
    button: Option<Box<dyn FnOnce(El) -> El>>,
}
impl Dialog {
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self { key: key.into(), title: title.into(), description: None, body: Vec::new(), open: false,
            outside: true, width: 480.0, fit: false, close_label: None, close_disabled: false, close: None, button: None }
    }
    pub fn open(mut self, open: bool) -> Self { self.open = open; self }
    pub fn description(mut self, description: impl Into<String>) -> Self { self.description = Some(description.into()); self }
    pub fn child(mut self, body: El) -> Self { self.body.push(body); self }
    pub fn width(mut self, width: f32) -> Self { self.width = width; self }
    /// Sizes the panel to its header and body instead of the standard 368 point height, for
    /// short dialogs such as a confirmation. The panel is capped at the window's height less its
    /// 16 point margin and its body shrinks; give a growing child a scroll view with
    /// `.shrink().min_h(0.0)` so it takes what is left and scrolls.
    pub fn fit(mut self) -> Self { self.fit = true; self }
    pub fn click_outside(mut self, close: bool) -> Self { self.outside = close; self }
    /// The accessible name for the header's close button (the web's `aria-label`), `"Close"` by
    /// default. Accessibility-only (not drawn); blank values fall back to `"Close"`.
    ///
    /// ```
    /// # use rune_kit::{Dialog, Kit};
    /// # struct App { open: bool }
    /// # let s = App { open: true };
    /// # let kit = Kit::neutral(true);
    /// # let mut layer = Vec::new();
    /// Dialog::new("confirm", "Confirmer")
    ///     .open(s.open)
    ///     .close_label("Fermer")
    ///     .on_close(|s: &mut App| s.open = false)
    ///     .build(&kit, &mut layer);
    /// ```
    pub fn close_label(mut self, label: impl Into<String>) -> Self { self.close_label = Some(label.into()); self }
    /// Disables dismissal while an action is in flight (such as saving): when `true` and
    /// [`Self::on_close`] is set, the header's close button stays visible in its disabled state
    /// and Escape and outside clicks are ignored. Without [`Self::on_close`], the close button
    /// stays omitted and dismissal remains disabled.
    ///
    /// ```
    /// # use rune_kit::{Dialog, Kit};
    /// # struct App { open: bool, saving: bool }
    /// # let s = App { open: true, saving: true };
    /// # let kit = Kit::neutral(true);
    /// # let mut layer = Vec::new();
    /// Dialog::new("save", "Saving changes")
    ///     .open(s.open)
    ///     .close_disabled(s.saving)
    ///     .on_close(|s: &mut App| s.open = false)
    ///     .build(&kit, &mut layer);
    /// ```
    pub fn close_disabled(mut self, disabled: bool) -> Self { self.close_disabled = disabled; self }
    /// Requests to dismiss the dialog: the header's close button, Escape, and outside clicks
    /// (when [`Self::click_outside`] is true). Without it the dialog still opens as a modal,
    /// leaves out the close button, and closes only from its own controls.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + Clone + 'static) -> Self {
        let button = f.clone();
        self.button = Some(Box::new(move |e| e.on_press(button)));
        // The policy is applied at build time, regardless of builder call order.
        self.close = Some(Box::new(move |e| e.modal(true, f)));
        self
    }
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) {
        if !self.open { return; }
        let dm = k.dialog_metrics();
        let title_font = dm.title.unwrap_or_else(|| {
            if self.description.is_some() { TextStyle { size: 15.0, tracking: -0.13, ..k.ty.ui_medium }.lh(20.0) }
            else { k.ty.ui_medium.lh(16.0) }
        });
        let mut title = col().grow(1.0).shrink().gap(4.0).child(text(self.title.clone())
            .font(title_font).color(k.c.text_base));
        if let Some(description) = self.description.clone() {
            title = title.child(text(description).font(k.ty.ui.lh(20.0)).color(k.c.text_muted).wrap().w_full());
        }
        let mut header = row().key("header").w_full().padding(dm.padding).items_start().child(title);
        if let Some(button) = self.button {
            let close_label = self.close_label
                .filter(|l| !l.trim().is_empty())
                .unwrap_or_else(|| "Close".into());
            let dismiss = IconButton::new(k.glyph.close).variant(IconButtonVariant::Ghost)
                .control_size(ControlSize::Regular).label(close_label).disabled(self.close_disabled)
                .build(k).key("close");
            let dismiss = if self.close_disabled { dismiss } else { button(dismiss) }
                .size(20.0, 20.0).mt(-2.0).mr(-2.0);
            header = header.child(dismiss);
        }
        let body = col().key("body").w_full().shrink().min_h(0.0).children(self.body);
        let (panel, body) = if self.fit { (col(), body) } else { (col().h(368.0), body.grow(1.0)) };
        let panel = panel.description(self.description.unwrap_or_default());
        let panel = k.elevate(panel.key(format!("{}-dialog", self.key)).role("dialog").label(self.title.clone())
            .w_full().max_w(self.width).shrink().min_h(0.0).radius(dm.radius).fill(k.c.layer_01)
            .child(header)
            .child(body)
            .token_motion(k.motion.overlay), &k.elevation.overlay);
        let panel = match (self.close, self.close_disabled) {
            (Some(close), false) => close(panel).modal_click_outside(self.outside),
            _ => panel.modal_trap(),
        };
        layer.push(el().key(scrim_key(&self.key)).absolute(0.0, 0.0).w_full().h_full()
            .fill(k.c.scrim).token_motion(k.motion.overlay).a11y_hidden());
        layer.push(col().key(format!("{}-layer", self.key)).absolute(0.0, 0.0).w_full().h_full().padding(16.0).center().child(panel));
    }
}
