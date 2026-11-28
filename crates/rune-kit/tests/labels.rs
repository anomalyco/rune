//! Tests that rune-kit's default accessible names and `TabWords` resolve through
//! `rune::set_labels`, while names set explicitly on a component still win.

use std::sync::{Mutex, MutexGuard};

use rune::{Cx, El, Env, Label, Runtime, TextEdit, col, el};
use rune_kit::shell::{RowStatus, TabItem, TabItemStatus, TabOrientation, TabWords};
use rune_kit::{
    Combobox, ComboboxEvent, ComboboxOption, ComboboxState, Dialog, ImageZoom, Kit, Loader,
    Popover, Preview, SidebarRow, Spinner, Stage, StageZoom, TabStrip, TextField, TextInput,
    Toast, root,
};

struct LabelsReset(#[allow(dead_code)] MutexGuard<'static, ()>);

impl Drop for LabelsReset {
    fn drop(&mut self) {
        rune::set_labels(|_| None);
    }
}

fn lock_labels() -> LabelsReset {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    rune::set_labels(|_| None);
    LabelsReset(guard)
}

fn install_german_labels() {
    rune::set_labels(|l| match l {
        Label::Close => Some("Schließen".into()),
        Label::Copy => Some("Kopieren".into()),
        Label::Clear => Some("Leeren".into()),
        Label::CloseTab => Some("Tab schließen".into()),
        Label::Working => Some("Arbeitet".into()),
        Label::NeedsAttention => Some("Aufmerksamkeit".into()),
        Label::Unread => Some("Ungelesen".into()),
        Label::Failed => Some("Fehlgeschlagen".into()),
        Label::Loading => Some("Wird geladen".into()),
        Label::Search(None) => Some("Suchen".into()),
        Label::Search(Some(name)) => Some(format!("{name} durchsuchen")),
        Label::ZoomIn => Some("Vergrößern".into()),
        Label::ZoomOut => Some("Verkleinern".into()),
        Label::ZoomToFit => Some("Anpassen".into()),
        Label::ContextMenu => Some("Kontextmenü".into()),
        _ => None,
    });
}

#[test]
fn kit_accessible_names_and_tab_words_follow_set_labels_and_respect_overrides() {
    let _lock = lock_labels();
    install_german_labels();

    struct S {
        edit_clear: TextEdit,
        edit_copy: TextEdit,
        field_copy: TextEdit,
        combo_default: ComboboxState,
        combo_custom: ComboboxState,
        zoom_default: ImageZoom,
        zoom_custom: ImageZoom,
        stage: StageZoom,
    }

    impl Default for S {
        fn default() -> Self {
            let mut combo_default = ComboboxState::default();
            combo_default.step(ComboboxEvent::Toggle { current: None });
            let mut combo_custom = ComboboxState::default();
            combo_custom.step(ComboboxEvent::Toggle { current: None });
            Self {
                edit_clear: TextEdit::new("hello"),
                edit_copy: TextEdit::new("secret"),
                field_copy: TextEdit::new("token"),
                combo_default,
                combo_custom,
                zoom_default: ImageZoom::new(),
                zoom_custom: ImageZoom::new(),
                stage: StageZoom::default(),
            }
        }
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let options = [ComboboxOption::new("m1", "Model 1")];

        // Dialog default vs explicit close_label.
        Dialog::new("dlg-default", "Titel")
            .open(true)
            .on_close(|_: &mut S| {})
            .build(&k, &mut layer);
        Dialog::new("dlg-custom", "Titel")
            .open(true)
            .close_label("Abbrechen")
            .on_close(|_: &mut S| {})
            .build(&k, &mut layer);

        // Popover with title (mounts close button).
        let pop = Popover::new("pop")
            .trigger(el().key("pop-t").size(20.0, 20.0))
            .title("Details")
            .open(true)
            .build(&k, &mut layer);

        // Combobox default vs explicit search_label / search_placeholder.
        let cb_def = Combobox::new("cb-def", &s.combo_default, &options, |s: &mut S| &mut s.combo_default)
            .label("Modelle")
            .build(&k, &mut layer);
        let cb_cus = Combobox::new("cb-cus", &s.combo_custom, &options, |s: &mut S| &mut s.combo_custom)
            .label("Modelle")
            .search_placeholder("Filter…")
            .search_label("Eigene Suche")
            .build(&k, &mut layer);

        // Toast with close button.
        let toast = Toast::new("t1", "Gespeichert").closable().build(&k);

        // Sidebar rows with statuses and close button.
        let sb_busy = SidebarRow::new("Build").status(RowStatus::Busy).on_close(|_: &mut S, _| {}).build(&k).key("sb-busy");
        let sb_attn = SidebarRow::new("Review").status(RowStatus::Attention).build(&k).key("sb-attn");
        let sb_unread = SidebarRow::new("Chat").status(RowStatus::Unread).build(&k).key("sb-unread");

        // TextInput clear & copy, TextField copy.
        let ti_clear = TextInput::new(&s.edit_clear).clear_button().build(&k).key("ti-clear");
        let ti_copy = TextInput::new(&s.edit_copy).copy_button().build(&k).key("ti-copy");
        let tf_copy = TextField::new("API Key", &s.field_copy).copyable().build(&k).key("tf-copy");

        // Spinner & Loader.
        let spin = Spinner::new().build(&k).key("spin");
        let load = Loader::new().build(&k).key("load");

        // TabStrip default TabWords vs explicit partial override.
        let tabs_def = TabStrip::new("tabs-def", TabOrientation::Horizontal).tabs(vec![
            TabItem::new("a", "One").status(Some(TabItemStatus::Busy)),
            TabItem::new("b", "Two").status(Some(TabItemStatus::Attention)),
            TabItem::new("c", "Three").status(Some(TabItemStatus::Unread)),
            TabItem::new("d", "Four").status(Some(TabItemStatus::Failed)),
        ]).on_close(|_: &mut S, _, _| {}).build(&k);
        let tabs_cus = TabStrip::new("tabs-cus", TabOrientation::Horizontal).tabs(vec![
            TabItem::new("x", "Custom").status(Some(TabItemStatus::Busy)),
        ]).words(TabWords { close: "Tab zu", busy: "Läuft", ..TabWords::default() })
            .on_close(|_: &mut S, _, _| {}).build(&k);

        // ImageZoom default vs custom labels + Stage default labels.
        let iz_def = Preview::new(|s: &mut S| &mut s.zoom_default, [100.0, 100.0], el().size(100.0, 100.0))
            .build(&s.zoom_default, &k, [400.0, 300.0], true).key("iz-def");
        let iz_cus = Preview::new(|s: &mut S| &mut s.zoom_custom, [100.0, 100.0], el().size(100.0, 100.0))
            .labels("Kleiner", "Passen", "Größer", "Weg")
            .build(&s.zoom_custom, &k, [400.0, 300.0], true).key("iz-cus");
        let stage = Stage::new(|s: &mut S| &mut s.stage, [100.0, 100.0], el().size(100.0, 100.0))
            .build(&s.stage, &k, [300.0, 200.0]).key("stage");

        let body = col().w_full().h_full().gap(4.0)
            .child(pop).child(cb_def).child(cb_cus).child(toast)
            .child(sb_busy).child(sb_attn).child(sb_unread)
            .child(ti_clear).child(ti_copy).child(tf_copy)
            .child(spin).child(load)
            .child(tabs_def).child(tabs_cus)
            .child(iz_def).child(iz_cus).child(stage);
        root(body, layer)
    }

    let mut rt = Runtime::new(S::default(), view, (800.0, 900.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let by_path = |needle: &str| els.iter().find(|e| e.path.contains(needle)).unwrap_or_else(|| panic!("missing element containing {needle:?}"));

    // Dialog close buttons.
    assert_eq!(by_path("dlg-default-dialog/header/close").label.as_deref(), Some("Schließen"));
    assert_eq!(by_path("dlg-custom-dialog/header/close").label.as_deref(), Some("Abbrechen"));

    // Popover close button.
    assert_eq!(by_path("pop-content/header/close").label.as_deref(), Some("Schließen"));

    // Toast close button.
    assert_eq!(by_path("t1/top/close").label.as_deref(), Some("Schließen"));

    // Sidebar statuses & close button.
    assert_eq!(by_path("sb-busy/status/close").label.as_deref(), Some("Schließen"));
    assert_eq!(by_path("sb-busy").description.as_deref(), Some("Arbeitet"));
    assert_eq!(by_path("sb-attn").description.as_deref(), Some("Aufmerksamkeit"));
    assert_eq!(by_path("sb-unread").description.as_deref(), Some("Ungelesen"));

    // TextInput clear/copy & TextField copy.
    assert_eq!(by_path("ti-clear/action").label.as_deref(), Some("Leeren"));
    assert_eq!(by_path("ti-copy/action").label.as_deref(), Some("Kopieren"));
    assert_eq!(by_path("tf-copy/wrapper/copy").label.as_deref(), Some("Kopieren"));

    // Spinner & Loader.
    assert_eq!(by_path("spin").label.as_deref(), Some("Wird geladen"));
    assert_eq!(by_path("load").label.as_deref(), Some("Wird geladen"));

    // Combobox search field (default vs custom).
    let cb_def_field = els.iter().find(|e| e.path.contains("cb-def-content") && e.label.as_deref() == Some("Modelle durchsuchen"));
    assert!(cb_def_field.is_some(), "Combobox default search_label should resolve Label::Search(Some(..))");
    let cb_cus_field = els.iter().find(|e| e.path.contains("cb-cus-content") && e.label.as_deref() == Some("Eigene Suche"));
    assert!(cb_cus_field.is_some(), "Combobox explicit search_label must win");

    // TabStrip default TabWords vs explicit overrides.
    let tab = |strip: &str, key: &str| els.iter().find(|e| e.path.contains(strip) && e.key.as_deref() == Some(key)).unwrap();
    assert_eq!(tab("tabs-def", "t-a").description.as_deref(), Some("Arbeitet"));
    assert_eq!(tab("tabs-def", "t-b").description.as_deref(), Some("Aufmerksamkeit"));
    assert_eq!(tab("tabs-def", "t-c").description.as_deref(), Some("Ungelesen"));
    assert_eq!(tab("tabs-def", "t-d").description.as_deref(), Some("Fehlgeschlagen"));
    let tab_close_def = els.iter().find(|e| e.path.contains("tabs-def") && e.path.contains("t-a/") && e.role == "button").unwrap();
    assert_eq!(tab_close_def.label.as_deref(), Some("Tab schließen"));
    assert_eq!(tab("tabs-cus", "t-x").description.as_deref(), Some("Läuft"));
    let tab_close_cus = els.iter().find(|e| e.path.contains("tabs-cus") && e.path.contains("t-x/") && e.role == "button").unwrap();
    assert_eq!(tab_close_cus.label.as_deref(), Some("Tab zu"));

    // ImageZoom controls (default vs custom).
    assert_eq!(by_path("iz-def").label.as_deref(), None);
    assert_eq!(els.iter().find(|e| e.path.contains("iz-def") && e.key.as_deref() == Some("image-preview-zoom-out")).unwrap().label.as_deref(), Some("Verkleinern"));
    assert_eq!(els.iter().find(|e| e.path.contains("iz-def") && e.key.as_deref() == Some("image-preview-zoom-in")).unwrap().label.as_deref(), Some("Vergrößern"));
    assert_eq!(els.iter().find(|e| e.path.contains("iz-def") && e.key.as_deref() == Some("image-preview-close")).unwrap().label.as_deref(), Some("Schließen"));
    assert_eq!(els.iter().find(|e| e.path.contains("iz-def") && e.help.as_deref() == Some("Anpassen")).is_some(), true);
    assert_eq!(els.iter().find(|e| e.path.contains("iz-cus") && e.key.as_deref() == Some("image-preview-zoom-out")).unwrap().label.as_deref(), Some("Kleiner"));
    assert_eq!(els.iter().find(|e| e.path.contains("iz-cus") && e.key.as_deref() == Some("image-preview-zoom-in")).unwrap().label.as_deref(), Some("Größer"));
    assert_eq!(els.iter().find(|e| e.path.contains("iz-cus") && e.key.as_deref() == Some("image-preview-close")).unwrap().label.as_deref(), Some("Weg"));

    // Stage zoom controls follow Label::ZoomIn / Label::ZoomOut.
    assert!(els.iter().any(|e| e.path.contains("stage") && e.label.as_deref() == Some("Vergrößern")));
    assert!(els.iter().any(|e| e.path.contains("stage") && e.label.as_deref() == Some("Verkleinern")));
}

#[test]
fn drawn_popup_menu_uses_context_menu_and_key_labels() {
    let _lock = lock_labels();
    install_german_labels();
    rune::set_labels(|l| match l {
        Label::ContextMenu => Some("Kontextmenü".into()),
        Label::Key("Ctrl") => Some("Strg".into()),
        Label::Key("Shift") => Some("Umschalt".into()),
        _ => None,
    });

    if rune::popup_menu_supported() {
        assert_eq!(rune::labels::label(Label::ContextMenu), "Kontextmenü");
        return;
    }

    use rune::{App, AppSpec, PopupItem, PopupMenu, PopupMenuRequest, Tx};
    use rune_kit::Menu;

    #[derive(Default)]
    struct Demo { popup: Option<PopupMenuRequest> }
    fn view(s: &Demo, _: &mut Cx<Demo>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        if let Some(req) = &s.popup {
            let _ = Menu::from_popup_menu("pm", req, |s: &mut Demo, picked, tx: &mut Tx| {
                s.popup = None;
                tx.resolve_popup_menu(picked);
            }).build(&k, &mut layer);
        }
        root(el().w_full().h_full(), layer)
    }
    let mut rt = App::new(AppSpec::new("kit-popup-labels", (400.0, 300.0), Demo::default, view))
        .on_popup_menu(|s: &mut Demo, req: PopupMenuRequest, _| s.popup = Some(req))
        .headless_runtime((400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| {
        let menu = PopupMenu::new().item(PopupItem::new("save", "Speichern").key_equivalent("ctrl+shift+s".parse().unwrap()));
        tx.popup_menu(menu, (40.0, 40.0), |_: &mut Demo, _, _| {});
    });
    rt.frame(0.0);
    let menu_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pm-menu")).unwrap();
    assert_eq!(menu_el.label.as_deref(), Some("Kontextmenü"));
    let sc_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("shortcut")).unwrap();
    assert_eq!(sc_el.text.as_deref(), Some(if cfg!(target_os = "macos") { "⌃⇧S" } else { "Strg+Umschalt+S" }));
}
