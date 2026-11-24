//! Field (`@opencode/ui` forms/field): label (with optional info icon), prefix, control, suffix.

use rune::{El, Icon, col, icon, row, text};

use crate::Kit;

pub static INFO: Icon = Icon {
    name: "kit-field-info",
    view: 16.0,
    body: r#"<path fill-rule="evenodd" d="M13 13H3V3H13V13ZM6.46777 6.81641V7.81641H7.5791V11.3721H8.5791V6.81641H6.46777ZM7.30078 4.62891V5.62891H8.85645V4.62891H7.30078Z" fill="currentColor"/>"#,
};

/// A labelled form field (`@opencode/ui` `forms/field`): draws a visible label, an optional
/// [`Self::info`] icon, wrapped [`Self::prefix`] and [`Self::suffix`] help lines, and sets the
/// outer `group`'s accessible name and description.
///
/// Wrap [`crate::TextInput`], [`crate::TextArea`] or [`crate::Select`] in `Field` for new forms
/// instead of the older [`crate::TextField`]:
///
/// - [`crate::TextInput::label`], [`crate::TextInput::description`], [`crate::TextArea::label`]
///   and [`crate::TextArea::description`] are accessibility-only and are not drawn; still pass `.label(..)`
///   on the inner control so assistive technology names the editor itself.
/// - [`crate::TextInput`] and [`crate::TextArea`] default to a 280 pt width; pass `.width(None)`
///   to fill the `Field`.
/// - Mark validation errors with `.invalid(true)` on the inner control alongside error text.
///
/// ```
/// # use rune::{TextEdit, Tx};
/// # use rune_kit::{Field, Kit, TextInput};
/// # #[derive(Default)]
/// # struct App { host: TextEdit, password: TextEdit, error: Option<String> }
/// # impl App { fn connect(&mut self, _: &mut Tx) {} }
/// # let s = App::default();
/// # let k = &Kit::neutral(true);
/// let host = Field::new()
///     .label("Host")
///     .info()
///     .prefix("Hostname or IP of the remote server.")
///     .control(
///         TextInput::new(&s.host)
///             .label("Host")
///             .placeholder("api.example.com")
///             .width(None)
///             .on_edit(|s: &mut App| &mut s.host)
///             .on_submit(|s: &mut App, tx: &mut Tx| s.connect(tx))
///             .build(k),
///     )
///     .build(k);
///
/// let password = Field::new()
///     .label("Password")
///     .control(
///         TextInput::new(&s.password)
///             .label("Password")
///             .placeholder("Required")
///             .width(None)
///             .secure()
///             .invalid(s.error.is_some())
///             .on_edit(|s: &mut App| &mut s.password)
///             .on_change(|s: &mut App, _: &mut Tx| s.error = None)
///             .on_submit(|s: &mut App, tx: &mut Tx| s.connect(tx))
///             .build(k),
///     )
///     .suffix(s.error.as_deref().unwrap_or("Stored in your system keychain."))
///     .build(k);
/// # let _ = (host, password);
/// ```
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
            |key: &str, s: String| text(s).key(key).w_full().min_h(11.0).font(k.ty.small.lh(11.0)).color(c.text_muted).wrap().a11y_hidden();
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
