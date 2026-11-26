use super::*;
use objc2_foundation::NSPoint;
use std::sync::atomic::AtomicUsize;

struct FakeStatusItem {
    image: RefCell<Option<Retained<NSImage>>>,
    titles: RefCell<Vec<String>>,
    tooltips: RefCell<Vec<Option<String>>>,
    highlights: RefCell<Vec<bool>>,
    rect: Cell<Option<[i32; 4]>>,
    popped_menus: RefCell<Vec<Retained<NSMenu>>>,
    removed: Cell<bool>,
    on_pop_up: RefCell<Option<Box<dyn Fn(&NSMenu)>>>,
}

impl FakeStatusItem {
    fn new() -> Self {
        Self {
            image: RefCell::new(None),
            titles: RefCell::new(Vec::new()),
            tooltips: RefCell::new(Vec::new()),
            highlights: RefCell::new(Vec::new()),
            rect: Cell::new(Some([2400, 0, 56, 48])),
            popped_menus: RefCell::new(Vec::new()),
            removed: Cell::new(false),
            on_pop_up: RefCell::new(None),
        }
    }
}

impl StatusItemHost for FakeStatusItem {
    fn set_image(&self, image: &NSImage) {
        *self.image.borrow_mut() = Some(Retained::from(image));
    }

    fn set_title(&self, title: &str) {
        self.titles.borrow_mut().push(title.to_owned());
    }

    fn set_tooltip(&self, tooltip: Option<&str>) {
        self.tooltips.borrow_mut().push(tooltip.map(str::to_owned));
    }

    fn set_highlighted(&self, highlighted: bool) {
        self.highlights.borrow_mut().push(highlighted);
    }

    fn screen_rect(&self, _mtm: MainThreadMarker) -> Option<[i32; 4]> {
        if self.removed.get() { None } else { self.rect.get() }
    }

    fn pop_up_menu(&self, menu: &NSMenu) {
        self.popped_menus.borrow_mut().push(Retained::from(menu));
        if let Some(cb) = self.on_pop_up.borrow().as_ref() {
            cb(menu);
        }
    }

    fn remove(&self) {
        self.removed.set(true);
    }
}

fn new_with_fake(
    mtm: MainThreadMarker,
    spec: TraySpec,
    thickness: f64,
    on_event: impl Fn(TrayEvent) + Send + Sync + 'static,
) -> Result<(Native, Rc<FakeStatusItem>, Retained<ButtonTarget>), TrayError> {
    let thickness = valid_thickness(thickness);
    let label = accessibility_label(&spec).to_owned();
    let image = make_image(&spec.icon, thickness, &label)?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let model = Arc::new(Mutex::new(Model {
        accessibility_label: label,
        title: String::new(),
        tooltip: spec.tooltip,
        icon: spec.icon,
        thickness,
        menu: spec.menu,
        rect: None,
        highlighted: false,
    }));
    let target = alloc_button_target(mtm, id, model.clone(), Arc::new(on_event));
    let fake = Rc::new(FakeStatusItem::new());
    let host: Rc<dyn StatusItemHost> = fake.clone();
    let native = install_native(mtm, id, model, target.clone(), host, &image);
    Ok((native, fake, target))
}

#[test]
fn make_image_sets_point_size_template_accessibility_and_premultiplied_pixels() {
    // 44x44 @2x template icon fits 22x22 pt menu bar.
    let icon = TrayIcon::rgba(44, 44, vec![255; 44 * 44 * 4]).template(true);
    let image = make_image(&icon, 22.0, "Rune Status").unwrap();
    assert_eq!(image.size(), NSSize::new(22.0, 22.0));
    assert!(image.isTemplate());
    assert_eq!(image.accessibilityDescription().map(|s| s.to_string()), Some("Rune Status".into()));
    let reps = image.representations();
    assert_eq!(reps.len(), 1);
    let rep = reps.objectAtIndex(0);
    assert_eq!(rep.pixelsWide(), 44);
    assert_eq!(rep.pixelsHigh(), 44);
    assert_eq!(rep.size(), NSSize::new(22.0, 22.0));

    // Wide 4x2 non-template icon preserves 2:1 aspect ratio -> 44x22 pt and premultiplies alpha.
    let mut pixels = vec![0u8; 4 * 2 * 4];
    // Pixel (0, 0): straight RGBA (200, 100, 50, 128) -> premultiplied (100, 50, 25, 128).
    pixels[0..4].copy_from_slice(&[200, 100, 50, 128]);
    // Pixel (1, 0): fully transparent straight RGBA (255, 255, 255, 0) -> (0, 0, 0, 0).
    pixels[4..8].copy_from_slice(&[255, 255, 255, 0]);
    let wide = TrayIcon::rgba(4, 2, pixels);
    let wide_img = make_image(&wide, 22.0, "").unwrap();
    assert_eq!(wide_img.size(), NSSize::new(44.0, 22.0));
    assert!(!wide_img.isTemplate());
    assert!(wide_img.accessibilityDescription().is_none());
    let rep = wide_img.representations().objectAtIndex(0);
    let bitmap = rep.downcast::<NSBitmapImageRep>().expect("NSBitmapImageRep");
    // SAFETY: bitmap owns 4x2x4 bytes with stride >= 16.
    let first_row = unsafe { std::slice::from_raw_parts(bitmap.bitmapData(), 8) };
    assert_eq!(&first_row[0..4], &[100, 50, 25, 128]);
    assert_eq!(&first_row[4..8], &[0, 0, 0, 0]);
}

#[test]
fn make_image_refuses_zero_or_mismatched_dimensions() {
    for (w, h, len) in [(0, 0, 0), (0, 16, 0), (16, 0, 0), (2, 2, 12)] {
        let icon = TrayIcon::rgba(w, h, vec![0; len]);
        assert!(matches!(make_image(&icon, 22.0, "T"), Err(TrayError::Backend(_))));
    }
}

#[test]
fn tray_new_off_main_thread_returns_backend_error() {
    let result = std::thread::spawn(|| {
        let spec = TraySpec::new("id", "Title", TrayIcon::rgba(16, 16, vec![255; 16 * 16 * 4]));
        super::super::Tray::new(spec, |_| {})
    })
    .join()
    .unwrap();
    assert!(matches!(result, Err(TrayError::Backend(_))));
}

#[test]
fn event_action_classifies_synthetic_nsevent_clicks() {
    let mk = |ty, flags| {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            ty,
            NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            0,
            None,
            0,
            1,
            1.0,
        )
        .unwrap()
    };
    assert_eq!(event_action(None), MacClickAction::Activate);
    assert_eq!(event_action(Some(&mk(NSEventType::LeftMouseUp, NSEventModifierFlags::empty()))), MacClickAction::Activate);
    assert_eq!(event_action(Some(&mk(NSEventType::RightMouseUp, NSEventModifierFlags::empty()))), MacClickAction::ShowMenu);
    assert_eq!(event_action(Some(&mk(NSEventType::LeftMouseUp, NSEventModifierFlags::Control))), MacClickAction::ShowMenu);
}

#[test]
fn fake_host_lifecycle_menu_tooltip_clicks_and_drop() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let events: Arc<Mutex<Vec<TrayEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let spec = TraySpec::new("dev.rune.test", "Test Tray", TrayIcon::rgba(32, 32, vec![255; 32 * 32 * 4]).template(true))
        .tooltip("Initial tip")
        .menu(vec![
            TrayItem::action("open", "Open"),
            TrayItem::Separator,
            TrayItem::action("disabled", "Disabled").enabled(false),
            TrayItem::action("quit", "Quit"),
        ]);

    let (native, fake, target) = new_with_fake(mtm, spec, 22.0, move |ev| {
        sink.lock().unwrap().push(ev);
    })
    .unwrap();

    // Image & initial tooltip applied.
    let img = fake.image.borrow().clone().expect("image set");
    assert_eq!(img.size(), NSSize::new(22.0, 22.0));
    assert!(img.isTemplate());
    assert_eq!(img.accessibilityDescription().map(|s| s.to_string()), Some("Test Tray".into()));
    assert_eq!(*fake.tooltips.borrow(), vec![Some("Initial tip".into())]);

    // Tooltip updates including clearing to empty -> None.
    native.set_tooltip("Updated tip".into());
    native.set_tooltip(String::new());
    assert_eq!(
        *fake.tooltips.borrow(),
        vec![Some("Initial tip".into()), Some("Updated tip".into()), None]
    );

    // Primary click sends Activate { token: None }.
    target.handle_click(mtm, MacClickAction::Activate);
    assert_eq!(*events.lock().unwrap(), vec![TrayEvent::Activate { token: None }]);

    // Secondary click builds and pops the flat NSMenu.
    target.handle_click(mtm, MacClickAction::ShowMenu);
    assert_eq!(fake.popped_menus.borrow().len(), 1);
    let menu = fake.popped_menus.borrow()[0].clone();
    assert!(!menu.autoenablesItems());
    assert_eq!(menu.numberOfItems(), 4);
    let open = menu.itemAtIndex(0).unwrap();
    let sep = menu.itemAtIndex(1).unwrap();
    let disabled = menu.itemAtIndex(2).unwrap();
    let quit = menu.itemAtIndex(3).unwrap();
    assert_eq!(open.title().to_string(), "Open");
    assert!(open.isEnabled());
    assert!(sep.isSeparatorItem());
    assert_eq!(disabled.title().to_string(), "Disabled");
    assert!(!disabled.isEnabled());
    assert_eq!(quit.title().to_string(), "Quit");
    assert!(quit.isEnabled());

    // Invoking action on enabled items fires Menu(id); disabled item does not.
    // SAFETY: invoking the ObjC target/action installed on the main-thread test menu items.
    unsafe {
        let open_target = open.target().unwrap();
        let _: () = msg_send![&*open_target, runeTrayMenuPick: &*open];
        let disabled_target = disabled.target().unwrap();
        let _: () = msg_send![&*disabled_target, runeTrayMenuPick: &*disabled];
        let quit_target = quit.target().unwrap();
        let _: () = msg_send![&*quit_target, runeTrayMenuPick: &*quit];
    }
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            TrayEvent::Activate { token: None },
            TrayEvent::Menu("open".into()),
            TrayEvent::Menu("quit".into()),
        ]
    );

    // Updating the menu changes subsequent pop-ups and clears old menu targets.
    native.set_menu(vec![TrayItem::action("resume", "Resume")]);
    target.handle_click(mtm, MacClickAction::ShowMenu);
    assert_eq!(fake.popped_menus.borrow().len(), 2);
    assert!(open.target().is_none(), "previous menu targets cleared on replacement");
    let new_menu = fake.popped_menus.borrow()[1].clone();
    assert_eq!(new_menu.numberOfItems(), 1);
    assert_eq!(new_menu.itemAtIndex(0).unwrap().title().to_string(), "Resume");

    // Empty menu does not pop up.
    native.set_menu(Vec::new());
    target.handle_click(mtm, MacClickAction::ShowMenu);
    assert_eq!(fake.popped_menus.borrow().len(), 2);

    // Drop removes the status item and clears the latest menu's targets.
    assert!(!fake.removed.get());
    drop(native);
    assert!(fake.removed.get());
    assert!(new_menu.itemAtIndex(0).unwrap().target().is_none());
}

#[test]
fn callback_panic_is_caught_and_reentrant_menu_click_is_ignored() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let spec = TraySpec::new("dev.rune.panic", "Panic", TrayIcon::rgba(16, 16, vec![255; 16 * 16 * 4]))
        .menu(vec![TrayItem::action("a", "A")]);
    let (native, fake, target) = new_with_fake(mtm, spec, 22.0, move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        panic!("boom");
    })
    .unwrap();

    // Panicking Activate callback does not unwind.
    target.handle_click(mtm, MacClickAction::Activate);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Re-entrant click while pop_up_menu is active is ignored.
    let target_for_cb = target.clone();
    *fake.on_pop_up.borrow_mut() = Some(Box::new(move |_| {
        target_for_cb.handle_click(mtm, MacClickAction::ShowMenu);
    }));
    target.handle_click(mtm, MacClickAction::ShowMenu);
    assert_eq!(fake.popped_menus.borrow().len(), 1);
    drop(native);
}

#[test]
fn fake_host_set_icon_set_title_rect_and_check_menu_items() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let events: Arc<Mutex<Vec<TrayEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let spec = TraySpec::new(
        "dev.rune.live",
        "Live Status",
        TrayIcon::rgba(32, 32, vec![255; 32 * 32 * 4]).template(true),
    )
    .menu(vec![
        TrayItem::action("open", "Open"),
        TrayItem::check("hud", "Show HUD", false),
        TrayItem::check("alert", "Alert", true),
        TrayItem::check("locked", "Locked", true).enabled(false),
    ]);

    let (native, fake, target) = new_with_fake(mtm, spec, 22.0, move |ev| {
        sink.lock().unwrap().push(ev);
    })
    .unwrap();

    // Initial rect comes from host; live updates reflect immediately on main thread.
    assert_eq!(native.rect(), Some([2400, 0, 56, 48]));
    fake.rect.set(Some([2320, 0, 92, 48]));
    assert_eq!(native.rect(), Some([2320, 0, 92, 48]));

    // set_title updates host and supports clearing back to empty.
    assert!(fake.titles.borrow().is_empty(), "Tray::new keeps title text unset");
    native.set_title("42%".into());
    native.set_title(String::new());
    assert_eq!(*fake.titles.borrow(), vec!["42%".to_owned(), String::new()]);

    // set_icon_name is a clean no-op on macOS.
    native.set_icon_name("ignored-symbolic".into());

    // set_icon swaps the NSImage (updating size/template while preserving accessibility label).
    let wide_alert = TrayIcon::rgba(48, 24, vec![255, 64, 64, 255].repeat(48 * 24));
    native.set_icon(wide_alert).unwrap();
    let img = fake.image.borrow().clone().expect("updated image");
    assert_eq!(img.size(), NSSize::new(44.0, 22.0));
    assert!(!img.isTemplate());
    assert_eq!(img.accessibilityDescription().map(|s| s.to_string()), Some("Live Status".into()));

    // Invalid icons are rejected without replacing the current NSImage.
    assert!(matches!(native.set_icon(TrayIcon::rgba(0, 16, Vec::new())), Err(TrayError::Backend(_))));
    assert!(matches!(native.set_icon(TrayIcon::rgba(4, 4, vec![0; 8])), Err(TrayError::Backend(_))));
    assert_eq!(fake.image.borrow().as_ref().unwrap().size(), NSSize::new(44.0, 22.0));

    // Check menu items set NSMenuItem.state (1 = on, 0 = off) and dispatch enabled clicks.
    target.handle_click(mtm, MacClickAction::ShowMenu);
    let menu = fake.popped_menus.borrow()[0].clone();
    assert_eq!(menu.numberOfItems(), 4);
    let open = menu.itemAtIndex(0).unwrap();
    let hud = menu.itemAtIndex(1).unwrap();
    let alert = menu.itemAtIndex(2).unwrap();
    let locked = menu.itemAtIndex(3).unwrap();
    assert_eq!(open.state(), 0);
    assert_eq!(hud.state(), 0);
    assert!(hud.isEnabled());
    assert_eq!(alert.state(), 1);
    assert!(alert.isEnabled());
    assert_eq!(locked.state(), 1);
    assert!(!locked.isEnabled());

    // SAFETY: invoking target/action installed on the main-thread test menu items.
    unsafe {
        let hud_target = hud.target().unwrap();
        let _: () = msg_send![&*hud_target, runeTrayMenuPick: &*hud];
        let locked_target = locked.target().unwrap();
        let _: () = msg_send![&*locked_target, runeTrayMenuPick: &*locked];
    }
    assert_eq!(*events.lock().unwrap(), vec![TrayEvent::Menu("hud".into())]);

    // Flipping checked state via set_menu updates subsequent pop-ups.
    native.set_menu(vec![TrayItem::check("hud", "Show HUD", true)]);
    target.handle_click(mtm, MacClickAction::ShowMenu);
    let next_menu = fake.popped_menus.borrow()[1].clone();
    assert_eq!(next_menu.itemAtIndex(0).unwrap().state(), 1);

    // Reading rect off the main thread returns the cached reading without blocking.
    let native = Arc::new(native);
    let for_thread = native.clone();
    let off_main_rect = std::thread::spawn(move || for_thread.rect()).join().unwrap();
    assert_eq!(off_main_rect, Some([2320, 0, 92, 48]));

    // Removing all tray items on exit hides the status item immediately before Drop runs.
    remove_all_on_main();
    assert!(fake.removed.get(), "remove_all must remove live status items immediately");
    assert_eq!(native.rect(), None);

    drop(native);
}

#[test]
fn set_highlighted_survives_icon_title_and_menu_updates() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let spec = TraySpec::new("dev.rune.pill", "Pill", TrayIcon::rgba(16, 16, vec![255; 16 * 16 * 4]))
        .menu(vec![TrayItem::action("a", "A")]);
    let (native, fake, target) = new_with_fake(mtm, spec, 22.0, |_| {}).unwrap();

    // Not highlighted: live updates leave the button alone.
    native.set_title("1".into());
    native.set_icon(TrayIcon::rgba(16, 16, vec![0; 16 * 16 * 4])).unwrap();
    assert!(fake.highlights.borrow().is_empty());

    // Only synchronous calls are counted: the follow-up applies `set_highlighted` and the context
    // menu queue on the main queue land after the button's click tracking, outside this test.
    native.set_highlighted(true);
    native.set_title("2".into());
    native.set_icon(TrayIcon::rgba(16, 16, vec![255; 16 * 16 * 4])).unwrap();
    assert_eq!(*fake.highlights.borrow(), vec![true; 3]);
    target.handle_click(mtm, MacClickAction::ShowMenu);
    assert_eq!(fake.popped_menus.borrow().len(), 1);

    native.set_highlighted(false);
    native.set_title("3".into());
    assert_eq!(*fake.highlights.borrow(), vec![true, true, true, false]);
    drop(native);
}
