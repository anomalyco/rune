//! Field (`@opencode/ui` forms/field): label (with optional info icon), prefix, control, suffix.

use rune::{El, Icon, col, icon, row, text};

use crate::Kit;

pub static INFO: Icon = Icon {
    name: "kit-field-info",
    view: 16.0,
    body: r#"<path fill-rule="evenodd" d="M13 13H3V3H13V13ZM6.46777 6.81641V7.81641H7.5791V11.3721H8.5791V6.81641H6.46777ZM7.30078 4.62891V5.62891H8.85645V4.62891H7.30078Z" fill="currentColor"/>"#,
};

pub struct Field {
    label: Option<String>,
    info: bool,
    prefix: Option<String>,
    suffix: Option<String>,
    control: Option<El>,
}

impl Field {
    pub fn new() -> Field {
        Field { label: None, info: false, prefix: None, suffix: None, control: None }
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// Show the info icon after the label (its tooltip is a `Tooltip` the app mounts).
    pub fn info(mut self) -> Self {
        self.info = true;
        self
    }
    pub fn prefix(mut self, p: impl Into<String>) -> Self {
        self.prefix = Some(p.into());
        self
    }
    pub fn suffix(mut self, s: impl Into<String>) -> Self {
        self.suffix = Some(s.into());
        self
    }
    /// The field's control (a `TextInput`, `TextArea`, …), built with `invalid` set as needed.
    pub fn control(mut self, c: El) -> Self {
        self.control = Some(c);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        // A group named by the label and described by the help text around the control, which
        // a reader hears on entering it.
        let mut root = col().role("group").gap(8.0).w_full().items_start();
        let help: Vec<&str> = [self.prefix.as_deref(), self.suffix.as_deref()].into_iter().flatten().collect();
        root = root.description(help.join(". "));
        if let Some(l) = self.label {
            root = root.label(l.clone());
            let mut lab = row().key("label").w_full().min_h(16.0).gap(4.0).center_y().child(
                text(l).key("text").font(rune::TextStyle { line_height: 16.0, ..k.ty.ui_medium }).color(c.text_base),
            );
            if self.info {
                lab = lab.child(icon(&INFO, 16.0).key("info").color(c.icon_muted).a11y_hidden());
            }
            root = root.child(lab.a11y_hidden());
        }
        let aside =
            |key: &str, s: String| text(s).key(key).w_full().min_h(11.0).font(k.ty.small.lh(11.0)).color(c.text_muted).a11y_hidden();
        if let Some(p) = self.prefix {
            root = root.child(aside("prefix", p));
        }
        if let Some(control) = self.control {
            root = root.child(control);
        }
        if let Some(s) = self.suffix {
            root = root.child(aside("suffix", s));
        }
        root
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::new()
    }
}
