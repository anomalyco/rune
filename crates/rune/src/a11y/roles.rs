//! Single source of truth for Rune's accessibility role metadata: AccessKit role mapping,
//! compact `RoleTag` indexing, debug nameless-control warnings, default live-region politeness,
//! web DOM/ARIA mirror projection, and AccessKit role traits.

use accesskit::Role;

use crate::LiveRegion;

/// What an element's role string says, given what else the element is.
pub(crate) struct RoleCtx {
    pub pressable: bool,
    /// It takes keyboard focus (`El::focusable`, or a pressable control under `Cx::web_focus`).
    pub focusable: bool,
    pub input: bool,
    pub multiline: bool,
    pub secure: bool,
    pub scrolls: bool,
    pub modal: bool,
    pub default_button: bool,
    pub labelled: bool,
    pub keyed: bool,
    pub has_text: bool,
    /// It shows a picture or video (`El::pixels`, `El::video`).
    pub image: bool,
}

/// How a Rune role string resolves to an AccessKit `Role` given the element's context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AkRole {
    /// Always resolves to this AccessKit role.
    Fixed(Role),
    /// Button that promotes to `Role::DefaultButton` when `cx.default_button` is set.
    Button,
    /// Decorative icon: `Role::Image` when labelled or keyed, omitted otherwise.
    Icon,
    /// A rule between sections is decoration; one that drags (`cx.pressable`) is a splitter.
    Separator,
    /// Wrapper around a text field: kept as a group when keyed, omitted otherwise.
    TextBoxWrapper,
    /// Explicitly omitted from the tree (`"none"`, `"presentation"`).
    Omit,
    /// Plain layout container (`"group"`), resolved from context via `group(cx)`.
    Group,
    /// Role with no fixed AccessKit mapping (e.g. `"scrollbar"`): uses the unknown-role fallback.
    Fallback,
}

/// Canonical metadata for one Rune accessibility role string.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RoleSpec {
    /// Rune role string (`El::role`).
    pub name: &'static str,
    /// AccessKit mapping strategy.
    pub ak: AkRole,
    /// Warn in debug builds when focusable and nameless (`a11y::node::a11y_warn_nameless`).
    pub warn_nameless: bool,
    /// Default live-region politeness when `El::live_region` is unset.
    pub default_live: Option<LiveRegion>,
    /// Web DOM mirror/export container HTML tag (`main`, `nav`, `section`, `ul`, `div`, ...).
    pub container_tag: Option<&'static str>,
    /// Explicit ARIA role carried by a container `div` (tables, listbox, dialogs).
    pub container_aria: Option<&'static str>,
    /// ARIA role carried by a mirrored button when Rune's role is a control (`option`, `tab`, ...).
    pub web_control_role: Option<&'static str>,
    /// Web page text-selection: controls that keep the pointer even without their own handler.
    pub takes_pointer: bool,
}

impl RoleSpec {
    const fn new(name: &'static str, ak: AkRole) -> Self {
        Self {
            name,
            ak,
            warn_nameless: false,
            default_live: None,
            container_tag: None,
            container_aria: None,
            web_control_role: None,
            takes_pointer: false,
        }
    }

    const fn role(name: &'static str, role: Role) -> Self {
        Self::new(name, AkRole::Fixed(role))
    }

    const fn warn_nameless(mut self) -> Self {
        self.warn_nameless = true;
        self
    }

    const fn live(mut self, live: LiveRegion) -> Self {
        self.default_live = Some(live);
        self
    }

    const fn container(mut self, tag: &'static str) -> Self {
        self.container_tag = Some(tag);
        self
    }

    /// A role-carrying container `div` (listbox, dialogs, and table parts whose CSS display
    /// controls layout independently of HTML table semantics).
    const fn aria_container(mut self) -> Self {
        self.container_tag = Some("div");
        self.container_aria = Some(self.name);
        self
    }

    const fn web_control(mut self) -> Self {
        self.web_control_role = Some(self.name);
        self
    }

    const fn pointer(mut self) -> Self {
        self.takes_pointer = true;
        self
    }
}

/// Every known Rune role and its metadata.
///
/// The most frequent element roles sit at the front so `RoleTag::of` and `spec` match common
/// roles in a handful of comparisons. `el::rare::RoleTag` indexes directly into this table.
pub(crate) const ROLES: &[RoleSpec] = &[
    RoleSpec::new("group", AkRole::Group),
    RoleSpec::role("text", Role::Label),
    RoleSpec::new("button", AkRole::Button).warn_nameless().pointer(),
    RoleSpec::new("icon", AkRole::Icon),
    RoleSpec::role("image", Role::Image),
    RoleSpec::new("textbox", AkRole::TextBoxWrapper).pointer(),
    RoleSpec::role("slider", Role::Slider).warn_nameless().pointer(),
    RoleSpec::role("option", Role::ListBoxOption).warn_nameless().web_control().pointer(),
    RoleSpec::role("list", Role::List).container("ul"),
    RoleSpec::role("listbox", Role::ListBox).aria_container(),
    RoleSpec::role("dialog", Role::Dialog).aria_container(),
    RoleSpec::role("listitem", Role::ListItem).container("li"),
    RoleSpec::role("heading", Role::Heading),
    RoleSpec::role("tab", Role::Tab).warn_nameless().web_control().pointer(),
    RoleSpec::role("region", Role::Region).container("section"),
    RoleSpec::role("alert", Role::Alert).live(LiveRegion::Assertive),
    RoleSpec::role("mark", Role::Label),
    RoleSpec::role("switch", Role::Switch).warn_nameless().web_control().pointer(),
    RoleSpec::role("link", Role::Link).warn_nameless().pointer(),
    RoleSpec::role("menuitem", Role::MenuItem).warn_nameless().web_control().pointer(),
    RoleSpec::role("radio", Role::RadioButton).warn_nameless().web_control().pointer(),
    RoleSpec::role("menu", Role::Menu),
    RoleSpec::role("status", Role::Status).live(LiveRegion::Polite),
    RoleSpec::role("tree", Role::Tree),
    RoleSpec::role("main", Role::Main).container("main"),
    RoleSpec::role("row", Role::Row).aria_container(),
    RoleSpec::role("treeitem", Role::TreeItem).warn_nameless().web_control(),
    RoleSpec::role("checkbox", Role::CheckBox).warn_nameless().web_control().pointer(),
    RoleSpec::role("toolbar", Role::Toolbar),
    RoleSpec::role("radiogroup", Role::RadioGroup),
    RoleSpec::role("progressbar", Role::ProgressIndicator),
    RoleSpec::role("note", Role::Note),
    RoleSpec::role("combobox", Role::ComboBox).warn_nameless().pointer(),
    RoleSpec::role("gridcell", Role::GridCell),
    RoleSpec::role("code", Role::Code),
    RoleSpec::role("columnheader", Role::ColumnHeader).aria_container(),
    RoleSpec::new("separator", AkRole::Separator),
    RoleSpec::role("tablist", Role::TabList).aria_container(),
    RoleSpec::role("segment", Role::RadioButton).warn_nameless(),
    RoleSpec::role("window", Role::Window),
    RoleSpec::role("navigation", Role::Navigation).container("nav"),
    RoleSpec::role("grid", Role::Grid),
    RoleSpec::new("none", AkRole::Omit),
    RoleSpec::role("img", Role::Image),
    RoleSpec::role("tooltip", Role::Tooltip),
    RoleSpec::role("canvas", Role::Canvas),
    RoleSpec::role("document", Role::Document),
    RoleSpec::role("figure", Role::Figure).container("figure"),
    RoleSpec::role("table", Role::Table).aria_container(),
    RoleSpec::role("search", Role::Search),
    RoleSpec::new("searchbox", AkRole::TextBoxWrapper).pointer(),
    RoleSpec::role("complementary", Role::Complementary).container("aside"),
    RoleSpec::role("blockquote", Role::Blockquote),
    RoleSpec::role("menuitemcheckbox", Role::MenuItemCheckBox).warn_nameless().web_control().pointer(),
    RoleSpec::role("menuitemradio", Role::MenuItemRadio).warn_nameless().web_control().pointer(),
    RoleSpec::new("scrollbar", AkRole::Fallback).pointer(),
    RoleSpec::role("spinbutton", Role::SpinButton).warn_nameless().pointer(),
    RoleSpec::role("tabpanel", Role::TabPanel),
    RoleSpec::role("article", Role::Article).container("article"),
    RoleSpec::role("terminal", Role::Terminal),
    RoleSpec::role("banner", Role::Banner).container("header"),
    RoleSpec::role("form", Role::Form),
    RoleSpec::role("contentinfo", Role::ContentInfo).container("footer"),
    RoleSpec::role("alertdialog", Role::AlertDialog).aria_container(),
    RoleSpec::role("log", Role::Log).live(LiveRegion::Polite),
    // Rune component aliases and less frequent ARIA roles.
    RoleSpec::new("toggle", AkRole::Button).warn_nameless(),
    RoleSpec::new("swatch", AkRole::Button).warn_nameless(),
    RoleSpec::new("sidebar-toggle", AkRole::Button).warn_nameless(),
    RoleSpec::role("segmented", Role::RadioGroup),
    RoleSpec::role("section-header", Role::Heading),
    RoleSpec::role("label", Role::Label),
    RoleSpec::role("paragraph", Role::Paragraph),
    RoleSpec::role("sidebar-list", Role::List),
    RoleSpec::role("treegrid", Role::TreeGrid),
    RoleSpec::role("rowgroup", Role::RowGroup),
    RoleSpec::role("cell", Role::Cell).aria_container(),
    RoleSpec::role("rowheader", Role::RowHeader).aria_container(),
    RoleSpec::role("menubar", Role::MenuBar),
    RoleSpec::role("timer", Role::Timer),
    RoleSpec::role("meter", Role::Meter),
    RoleSpec::new("sidebar-grip", AkRole::Separator),
    RoleSpec::new("presentation", AkRole::Omit),
];

const _: () = assert!(ROLES.len() < u8::MAX as usize);

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() { return false; }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] { return false; }
        i += 1;
    }
    true
}

/// Index of `role` in `ROLES`, for compile-time and compact `RoleTag` lookup.
pub(crate) const fn index(role: &str) -> Option<u8> {
    let mut i = 0;
    while i < ROLES.len() {
        if str_eq(ROLES[i].name, role) { return Some(i as u8); }
        i += 1;
    }
    None
}

/// The canonical metadata for `role`, if it is a known Rune role.
#[inline]
pub(crate) fn spec(role: &str) -> Option<&'static RoleSpec> {
    ROLES.iter().find(|r| r.name == role)
}

/// The AccessKit role for `role`, or `None` when the element adds nothing to the tree and its
/// children belong to its parent (`"none"`, `"presentation"`, an unlabelled decorative icon,
/// a plain layout box).
pub(crate) fn map(role: &str, cx: &RoleCtx) -> Option<Role> {
    if cx.input {
        return Some(if cx.secure { Role::PasswordInput }
            else if role == "searchbox" { Role::SearchInput }
            else if role == "combobox" { Role::EditableComboBox }
            else if role == "spinbutton" { Role::SpinButton }
            else if cx.multiline { Role::MultilineTextInput } else { Role::TextInput });
    }
    match spec(role).map_or(AkRole::Fallback, |s| s.ak) {
        AkRole::Fixed(r) => Some(r),
        AkRole::Button => Some(if cx.default_button { Role::DefaultButton } else { Role::Button }),
        AkRole::Icon => (cx.labelled || cx.keyed).then_some(Role::Image),
        AkRole::Separator => cx.pressable.then_some(Role::Splitter),
        AkRole::TextBoxWrapper => cx.keyed.then_some(Role::Group),
        AkRole::Omit => None,
        AkRole::Group => group(cx),
        AkRole::Fallback => Some(if cx.pressable && cx.focusable { Role::Button } else { group(cx).unwrap_or(Role::Group) }),
    }
}

/// A plain box: a dialog when modal, a scroll view when it scrolls, a button when the keyboard
/// can press it, a label when it holds text; kept as a group when an app keyed or labelled it,
/// so drivers can find it by key; otherwise left out, its children joining its parent. A box
/// that only takes a pointer's press (a pane that clears a selection, a scrim that closes a
/// popover) is no button, as a `<div onclick>` isn't: it keeps its press as an action.
fn group(cx: &RoleCtx) -> Option<Role> {
    if cx.modal { return Some(Role::Dialog); }
    if cx.pressable && cx.focusable { return Some(if cx.default_button { Role::DefaultButton } else { Role::Button }); }
    if cx.scrolls { return Some(Role::ScrollView); }
    if cx.has_text { return Some(Role::Label); }
    if cx.image && (cx.keyed || cx.labelled) { return Some(Role::Image); }
    (cx.keyed || cx.labelled).then_some(Role::Group)
}

/// Whether debug builds warn when a focusable element with this role has no accessible name.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn warn_nameless(role: &str) -> bool {
    spec(role).is_some_and(|s| s.warn_nameless)
}

/// Default live-region politeness for `role` when `El::live_region` isn't explicitly set.
pub(crate) fn default_live(role: &str) -> Option<LiveRegion> {
    spec(role).and_then(|s| s.default_live)
}

/// The HTML element a landmark, list or role-carrying container stands for in the web DOM mirror
/// and static HTML export.
pub(crate) fn container_tag(role: &str) -> Option<&'static str> {
    spec(role).and_then(|s| s.container_tag)
}

/// The ARIA role a container `div` carries (listbox, dialogs, and table parts).
pub(crate) fn container_aria(role: &str) -> Option<&'static str> {
    spec(role).and_then(|s| s.container_aria)
}

/// The ARIA role a mirrored button carries when Rune's role is a control (a palette's option,
/// a tab, a switch, a checkbox, a radio, a menu item, a tree item).
pub(crate) fn web_control_role(role: &str) -> Option<&'static str> {
    spec(role).and_then(|s| s.web_control_role)
}

/// Roles of controls a press works, which keep the pointer in a web page even without their own
/// handler.
pub(crate) fn takes_pointer(role: &str) -> bool {
    spec(role).is_some_and(|s| s.takes_pointer)
}

/// Per-AccessKit-`Role` traits shared by the native tree, JSON dump, and web focus proxy.
#[derive(Clone, Copy)]
struct AkRoleMeta {
    named_from_content: bool,
    selectable_item: bool,
    leaf_control: bool,
    named_by_accesskit: bool,
    focus_role: &'static str,
}

impl AkRoleMeta {
    const DEFAULT: Self = Self {
        named_from_content: false,
        selectable_item: false,
        leaf_control: false,
        named_by_accesskit: false,
        focus_role: "group",
    };

    const fn focus(mut self, role: &'static str) -> Self { self.focus_role = role; self }
    const fn content_name(mut self) -> Self { self.named_from_content = true; self }
    const fn selectable(mut self) -> Self { self.selectable_item = true; self }
    const fn leaf(mut self) -> Self { self.leaf_control = true; self }
    const fn ak_named(mut self) -> Self { self.named_by_accesskit = true; self }
}

const fn ak_meta(role: Role) -> AkRoleMeta {
    const D: AkRoleMeta = AkRoleMeta::DEFAULT;
    match role {
        Role::Button => D.focus("button").leaf().ak_named(),
        Role::DefaultButton => D.focus("button").leaf().content_name(),
        Role::CheckBox => D.focus("checkbox").leaf().ak_named(),
        Role::Switch => D.focus("switch").leaf().content_name(),
        Role::RadioButton => D.focus("radio").leaf().ak_named(),
        Role::Link => D.focus("link").leaf().ak_named(),
        Role::Tab => D.focus("tab").leaf().content_name().selectable(),
        Role::ListBoxOption => D.focus("option").leaf().content_name().selectable(),
        Role::MenuListOption => D.focus("option").selectable(),
        Role::ListItem => D.focus("listitem").content_name().selectable(),
        Role::TreeItem => D.focus("treeitem").content_name().selectable(),
        Role::MenuItem => D.focus("menuitem").leaf().ak_named(),
        Role::MenuItemCheckBox => D.focus("menuitemcheckbox").leaf().content_name(),
        Role::MenuItemRadio => D.focus("menuitemradio").leaf().content_name(),
        Role::Row => D.focus("row").content_name().selectable(),
        Role::GridCell => D.focus("gridcell").content_name().selectable(),
        Role::Cell => D.focus("gridcell").content_name(),
        Role::ColumnHeader | Role::RowHeader | Role::Tooltip | Role::Status | Role::Alert => D.content_name(),
        Role::Heading => D.focus("heading").content_name(),
        Role::TextInput | Role::MultilineTextInput | Role::SearchInput | Role::PasswordInput | Role::Terminal => D.focus("textbox"),
        Role::Slider => D.focus("slider"),
        Role::SpinButton => D.focus("spinbutton"),
        Role::ComboBox | Role::EditableComboBox => D.focus("combobox"),
        Role::Dialog => D.focus("dialog"),
        Role::AlertDialog => D.focus("alertdialog"),
        Role::Region => D.focus("region"),
        Role::Image => D.focus("img"),
        _ => D,
    }
}

/// Roles whose name AccessKit doesn't take from their descendants' text, but a reader expects
/// to: Rune names them from the text inside when the app gave no label.
pub(crate) fn named_from_content(role: Role) -> bool {
    // Containers (groups, dialogs, toolbars) are named by their label alone: their content is
    // what a reader moves through.
    ak_meta(role).named_from_content
}

/// Roles whose items carry a selected state (UIA SelectionItem, AX selected).
pub(crate) fn selectable_item(role: Role) -> bool {
    ak_meta(role).selectable_item
}

/// Controls a reader hears as one thing: Chromium makes the content of a button, checkbox, radio
/// button, switch, tab and option one leaf (ARIA's "children presentational"), and a link's or a
/// menu item's text is its name.
pub(crate) fn leaf_control(role: Role) -> bool {
    ak_meta(role).leaf_control
}

/// Roles that take their name from the text inside them through AccessKit itself.
pub(crate) fn named_by_accesskit(role: Role) -> bool {
    ak_meta(role).named_by_accesskit
}

/// The ARIA role a browser's reader knows for an AccessKit role, for the web focus proxy.
pub(crate) fn focus_role(role: Role) -> &'static str {
    ak_meta(role).focus_role
}

/// A stable lower-case name for `role`, for `--tree` and goldens.
pub(crate) fn name(role: Role) -> String {
    let debug = format!("{role:?}");
    let mut out = String::with_capacity(debug.len() + 4);
    for (i, c) in debug.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 { out.push('-'); }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn cx() -> RoleCtx {
        RoleCtx { pressable: false, focusable: false, input: false, multiline: false, secure: false, scrolls: false, modal: false,
            default_button: false, labelled: false, keyed: false, has_text: false, image: false }
    }

    #[test]
    fn maps_aria_roles_and_rune_component_roles() {
        assert_eq!(map("button", &cx()), Some(Role::Button));
        assert_eq!(map("button", &RoleCtx { default_button: true, ..cx() }), Some(Role::DefaultButton));
        assert_eq!(map("switch", &cx()), Some(Role::Switch));
        assert_eq!(map("option", &cx()), Some(Role::ListBoxOption));
        assert_eq!(map("progressbar", &cx()), Some(Role::ProgressIndicator));
        assert_eq!(map("segmented", &cx()), Some(Role::RadioGroup));
        assert_eq!(map("none", &RoleCtx { pressable: true, ..cx() }), None);
        assert_eq!(map("scrollbar", &cx()), Some(Role::Group));
    }

    #[test]
    fn text_fields_take_their_kind_from_the_editor() {
        let field = RoleCtx { input: true, ..cx() };
        assert_eq!(map("textbox", &field), Some(Role::TextInput));
        assert_eq!(map("searchbox", &field), Some(Role::SearchInput));
        assert_eq!(map("combobox", &field), Some(Role::EditableComboBox));
        assert_eq!(map("spinbutton", &field), Some(Role::SpinButton));
        assert_eq!(map("combobox", &cx()), Some(Role::ComboBox));
        assert_eq!(map("textbox", &RoleCtx { multiline: true, ..field }), Some(Role::MultilineTextInput));
        assert_eq!(map("textbox", &RoleCtx { secure: true, ..field }), Some(Role::PasswordInput));
        assert_eq!(map("combobox", &RoleCtx { secure: true, ..field }), Some(Role::PasswordInput));
        assert_eq!(map("spinbutton", &RoleCtx { secure: true, ..field }), Some(Role::PasswordInput));
        assert_eq!(focus_role(Role::EditableComboBox), "combobox");
        assert_eq!(focus_role(Role::SpinButton), "spinbutton");
    }

    #[test]
    fn plain_boxes_join_their_parent_unless_they_mean_something() {
        assert_eq!(map("group", &cx()), None);
        assert_eq!(map("group", &RoleCtx { keyed: true, ..cx() }), Some(Role::Group));
        assert_eq!(map("group", &RoleCtx { scrolls: true, ..cx() }), Some(Role::ScrollView));
        assert_eq!(map("group", &RoleCtx { pressable: true, focusable: true, ..cx() }), Some(Role::Button));
        assert_eq!(map("group", &RoleCtx { pressable: true, ..cx() }), None);
        assert_eq!(map("group", &RoleCtx { modal: true, ..cx() }), Some(Role::Dialog));
        assert_eq!(map("icon", &cx()), None);
        assert_eq!(map("icon", &RoleCtx { labelled: true, ..cx() }), Some(Role::Image));
        assert_eq!(map("whatever", &RoleCtx { pressable: true, focusable: true, ..cx() }), Some(Role::Button));
        assert_eq!(map("whatever", &RoleCtx { pressable: true, ..cx() }), Some(Role::Group));
    }

    #[test]
    fn names_are_kebab_case() {
        assert_eq!(name(Role::ListBoxOption), "list-box-option");
        assert_eq!(name(Role::Button), "button");
    }

    #[test]
    fn canonical_table_is_unique_complete_and_consistent() {
        let mut seen = HashSet::new();
        for (i, row) in ROLES.iter().enumerate() {
            assert!(seen.insert(row.name), "duplicate role in ROLES: {}", row.name);
            assert_eq!(index(row.name), Some(i as u8), "index mismatch for {}", row.name);
            let tag = crate::el::RoleTag::of(row.name).expect("RoleTag covers every ROLES row");
            assert_eq!(tag.name(), Some(row.name));
            if let Some(aria) = row.container_aria {
                assert_eq!(row.container_tag, Some("div"), "container_aria role {} should use div tag", row.name);
                assert_eq!(aria, row.name);
            }
            if let Some(ctrl) = row.web_control_role {
                assert_eq!(ctrl, row.name);
                assert!(row.warn_nameless, "web control role {} should warn when nameless", row.name);
                if let AkRole::Fixed(ak) = row.ak {
                    assert_eq!(focus_role(ak), ctrl, "focus_role disagrees with web_control_role for {}", row.name);
                }
            }
            if let AkRole::Fixed(ak) = row.ak {
                assert!(!(named_from_content(ak) && named_by_accesskit(ak)),
                    "role {ak:?} cannot be named both by Rune and by AccessKit");
            }
        }
        assert!(!(named_from_content(Role::Button) && named_by_accesskit(Role::Button)));
        assert!(!(named_from_content(Role::DefaultButton) && named_by_accesskit(Role::DefaultButton)));
    }

    #[test]
    fn canonical_table_preserves_web_and_warning_projections() {
        let warn: Vec<&str> = ROLES.iter().filter(|r| r.warn_nameless).map(|r| r.name).collect();
        assert_eq!(warn.len(), 18);
        for name in ["button", "toggle", "swatch", "sidebar-toggle", "checkbox", "switch", "radio", "segment", "link",
            "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "treeitem", "slider", "combobox", "spinbutton"] {
            assert!(warn_nameless(name), "expected warn_nameless({name})");
        }
        let pointer: Vec<&str> = ROLES.iter().filter(|r| r.takes_pointer).map(|r| r.name).collect();
        assert_eq!(pointer.len(), 16);
        for name in ["button", "link", "tab", "checkbox", "switch", "radio", "combobox", "option",
            "menuitem", "menuitemcheckbox", "menuitemradio", "slider", "spinbutton", "scrollbar", "textbox", "searchbox"] {
            assert!(takes_pointer(name), "expected takes_pointer({name})");
        }
        assert_eq!(container_tag("navigation"), Some("nav"));
        assert_eq!(container_tag("banner"), Some("header"));
        assert_eq!(container_tag("contentinfo"), Some("footer"));
        assert_eq!(container_tag("region"), Some("section"));
        assert_eq!(container_tag("complementary"), Some("aside"));
        assert_eq!(container_tag("list"), Some("ul"));
        assert_eq!(container_tag("listitem"), Some("li"));
        assert_eq!(container_tag("dialog"), Some("div"));
        assert_eq!(container_aria("dialog"), Some("dialog"));
        assert_eq!(container_tag("tablist"), Some("div"));
        assert_eq!(container_aria("tablist"), Some("tablist"));
        assert_eq!(container_aria("table"), Some("table"));
        assert_eq!(container_aria("main"), None);
        assert_eq!(web_control_role("option"), Some("option"));
        assert_eq!(web_control_role("button"), None);
        assert_eq!(default_live("status"), Some(LiveRegion::Polite));
        assert_eq!(default_live("log"), Some(LiveRegion::Polite));
        assert_eq!(default_live("alert"), Some(LiveRegion::Assertive));
        assert_eq!(default_live("region"), None);
        assert_eq!(focus_role(Role::DefaultButton), "button");
        assert_eq!(focus_role(Role::Terminal), "textbox");
        assert_eq!(focus_role(Role::Cell), "gridcell");
        assert_eq!(focus_role(Role::Image), "img");
        assert_eq!(focus_role(Role::List), "group");
    }
}
