use super::*;
use crate::{AppSpec, el};

fn app() -> App<Vec<u32>> {
    App::new(AppSpec::new("menu", (100.0, 100.0), Vec::new, |_, _| el()))
}

fn item(id: &str) -> MenuItem<Vec<u32>> {
    MenuItem::new("Open Quick Panel", |s: &mut Vec<u32>, _| s.push(1)).id(id)
}

fn drain<S: 'static>(app: &App<S>, s: &mut S) {
    for update in app.shared.take().updates {
        update(s, &mut Tx::default());
    }
}

#[test]
fn title_and_id_dispatch_on_the_state_thread_with_reentrant_menu_updates() {
    let app = app()
        .menu_item(item("quick"))
        .menu_item(MenuItem::new("Dock", |s: &mut Vec<u32>, tx| { s.push(2); tx.quit(); }).dock());
    let _guard = MenuGuard::attach(&app.shared, true);
    let handle = app.handle();
    let shared = app.shared.clone();
    std::thread::spawn(move || fire(&shared, "Open Quick Panel").unwrap()).join().unwrap();
    let mut state = Vec::new();
    assert!(state.is_empty());
    drain(&app, &mut state);
    assert_eq!(state, [1]);
    fire(&app.shared, "quick").unwrap();
    drain(&app, &mut state);
    assert_eq!(state, [1, 1]);
    let h = handle.clone();
    handle.update(move |_, _| h.set_menu([item("replacement").checked(true)]).unwrap());
    drain(&app, &mut state);
    assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions[0].meta.id, "replacement");
}

#[test]
fn replacement_disable_and_shutdown_cancel_stale_actions() {
    let app = app().menu_item(item("quick"));
    let guard = MenuGuard::attach(&app.shared, true);
    let handle = app.handle();
    fire(&app.shared, "quick").unwrap();
    handle.set_menu([item("quick").enabled(false).checked(true)]).unwrap();
    let mut state = Vec::new();
    drain(&app, &mut state);
    assert!(state.is_empty());
    assert_eq!(fire(&app.shared, "quick"), Err(MenuError::Disabled("quick".into())));
    assert!(app.shared.menus.core.state.lock().unwrap().descriptions[0].meta.checked);
    handle.set_menu([item("quick")]).unwrap();
    fire(&app.shared, "quick").unwrap();
    drop(guard);
    drain(&app, &mut state);
    assert!(state.is_empty());
    assert_eq!(fire(&app.shared, "quick"), Err(MenuError::NotRunning));
}

#[test]
fn validation_is_atomic_and_ambiguous_titles_require_ids() {
    let app = app().menu_item(item("first")).menu_item(item("second").menu("Tools"));
    let _guard = MenuGuard::attach(&app.shared, true);
    assert_eq!(fire(&app.shared, "Open Quick Panel"), Err(MenuError::AmbiguousTitle("Open Quick Panel".into())));
    assert!(fire(&app.shared, "first").is_ok());
    assert!(matches!(app.handle().set_menu([item("duplicate"), item("duplicate")]), Err(MenuError::Invalid(_))));
    assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions.len(), 2);
    assert!(matches!(fire(&app.shared, "missing"), Err(MenuError::UnknownItem(_))));
    let invalid = self::app().menu_item(item("same")).menu_item(item("same"));
    let _guard = MenuGuard::attach(&invalid.shared, true);
    assert!(matches!(invalid.handle().take_menu_errors().as_slice(), [MenuError::Invalid(_)]));
    assert!(matches!(fire(&invalid.shared, "same"), Err(MenuError::UnknownItem(_))));
}

#[test]
fn separators_need_no_ids_and_never_fire() {
    let file = |title: &str| MenuItem::new(title, |s: &mut Vec<u32>, _| s.push(3)).menu("File");
    let app = app()
        .menu_item(file("New"))
        .menu_item(MenuItem::separator().menu("File"))
        .menu_item(file("Close"))
        .menu_item(MenuItem::separator().menu("File"))
        .menu_item(MenuItem::separator().dock());
    let _guard = MenuGuard::attach(&app.shared, true);
    assert!(app.handle().take_menu_errors().is_empty());
    assert_eq!(fire(&app.shared, ""), Err(MenuError::UnknownItem(String::new())));
    fire(&app.shared, "Close").unwrap();
    let mut state = Vec::new();
    drain(&app, &mut state);
    assert_eq!(state, [3]);
    app.handle().set_menu([file("New"), MenuItem::separator().menu("File"), MenuItem::separator().menu("File")]).unwrap();
    assert!(matches!(app.shared.menus.core.state.lock().unwrap().descriptions[1].kind, EntryKind::Separator));
    assert!(matches!(app.handle().set_menu([MenuItem::separator().menu(" ")]), Err(MenuError::Invalid(_))));
}

#[test]
fn equivalent_and_non_send_state_remain_builder_compatible() {
    fn send_sync<T: Send + Sync>() {}
    fn is_clone<T: Clone>() {}
    send_sync::<AppHandle<std::rc::Rc<()>>>();
    send_sync::<MenuItem<std::rc::Rc<()>>>();
    is_clone::<MenuItem<std::rc::Rc<()>>>();
    let shortcut: Shortcut = "cmd+shift+k".parse().unwrap();
    let item = item("quick").key_equivalent(shortcut.clone());
    assert_eq!(item.meta.equivalent, Some(shortcut));
    assert!(!item.meta.keys_first && item.keys_first().meta.keys_first);
}

#[test]
fn about_item_validates_and_dispatches_headlessly() {
    let about = |title: &str| MenuItem::about(title, |s: &mut Vec<u32>, _| s.push(7));
    let app = app().menu_item(about("About Demo").id("about")).menu_item(item("quick"));
    let _guard = MenuGuard::attach(&app.shared, true);
    assert!(app.handle().take_menu_errors().is_empty());
    {
        let state = app.shared.menus.core.state.lock().unwrap();
        assert!(state.descriptions[0].meta.replaces_about);
        assert_eq!(state.descriptions[0].meta.location, Location::App);
        assert!(!state.descriptions[1].meta.replaces_about);
    }
    fire(&app.shared, "about").unwrap();
    fire(&app.shared, "About Demo").unwrap();
    let mut state = Vec::new();
    drain(&app, &mut state);
    assert_eq!(state, [7, 7]);

    // Duplicate About, moved out of App menu, after, alternate, or empty title are rejected atomically.
    for bad in [
        vec![about("About One"), about("About Two")],
        vec![about("About File").menu("File")],
        vec![about("About Dock").dock()],
        vec![about("About After").after("quick")],
        vec![about("About Alt").alternate()],
        vec![about("   ")],
    ] {
        assert!(matches!(app.handle().set_menu(bad), Err(MenuError::Invalid(_))));
    }
    assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions.len(), 2);

    // Clearing removes the About item; setting a fresh About item works.
    app.handle().set_menu([]).unwrap();
    assert!(app.shared.menus.core.state.lock().unwrap().descriptions.is_empty());
    app.handle().set_menu([about("About Again")]).unwrap();
    assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions[0].meta.id, "About Again");

    // Invalid startup snapshot with duplicate About records MenuError::Invalid.
    let invalid = self::app().menu_item(about("About One")).menu_item(about("About Two"));
    let _guard = MenuGuard::attach(&invalid.shared, true);
    assert!(matches!(invalid.handle().take_menu_errors().as_slice(), [MenuError::Invalid(_)]));
}

#[test]
fn with_action_captures_per_item_data_clones_and_supports_non_send_state() {
    struct LocalState {
        _not_send: std::rc::Rc<()>,
        picked: Vec<String>,
    }
    let app = App::new(AppSpec::new(
        "closure-menu",
        (100.0, 100.0),
        || LocalState { _not_send: std::rc::Rc::new(()), picked: Vec::new() },
        |_, _| el(),
    ));
    let mut built = Vec::new();
    for host in ["prod", "staging", "dev"] {
        let name = host.to_owned();
        built.push(
            MenuItem::with_action(host, move |s: &mut LocalState, _| {
                s.picked.push(name.clone());
            })
            .id(format!("host-{host}"))
            .menu("Session"),
        );
    }
    // Cloning a closure item preserves its captured action.
    let cloned_prod = built[0].clone().id("host-prod-copy").dock();
    let app = built.into_iter().fold(app, App::menu_item).menu_item(cloned_prod);
    let _guard = MenuGuard::attach(&app.shared, true);
    assert!(app.handle().take_menu_errors().is_empty());

    let mut state = LocalState { _not_send: std::rc::Rc::new(()), picked: Vec::new() };
    fire(&app.shared, "host-staging").unwrap();
    fire(&app.shared, "host-prod-copy").unwrap();
    fire(&app.shared, "dev").unwrap();
    drain(&app, &mut state);
    assert_eq!(state.picked, ["staging", "prod", "dev"]);

    // Re-entrant set_menu from inside a with_action closure does not deadlock and replaces items.
    let handle = app.handle();
    let handle_for_cb = handle.clone();
    handle.set_menu([
        MenuItem::with_action("Swap", move |s: &mut LocalState, _| {
            s.picked.push("swapped".into());
            handle_for_cb.set_menu([
                MenuItem::with_action("After Swap", |s: &mut LocalState, _| {
                    s.picked.push("after".into());
                }),
            ]).unwrap();
        }),
    ]).unwrap();
    fire(&app.shared, "Swap").unwrap();
    drain(&app, &mut state);
    assert_eq!(state.picked, ["staging", "prod", "dev", "swapped"]);
    assert_eq!(fire(&app.shared, "Swap"), Err(MenuError::UnknownItem("Swap".into())));
    fire(&app.shared, "After Swap").unwrap();
    drain(&app, &mut state);
    assert_eq!(state.picked, ["staging", "prod", "dev", "swapped", "after"]);
}

#[test]
fn submenu_nesting_lookup_disabled_ancestors_and_validation() {
    let app = App::new(AppSpec::new("submenus", (100.0, 100.0), Vec::<String>::new, |_, _| el()));
    let leaf = |title: &str, id: &str| {
        let label = id.to_owned();
        MenuItem::with_action(title, move |s: &mut Vec<String>, _| s.push(label.clone())).id(id)
    };
    let app = app
        .menu_item(
            MenuItem::submenu("Attach", [
                MenuItem::submenu("prod", [
                    leaf("main", "prod:main"),
                    MenuItem::separator(),
                    leaf("logs", "prod:logs").enabled(false),
                ]),
                MenuItem::submenu("staging", [
                    leaf("main", "staging:main"),
                    leaf("api", "staging:api"),
                ]),
                MenuItem::submenu("offline", [
                    MenuItem::submenu("deep", [
                        leaf("unreachable", "offline:deep:unreachable"),
                    ]),
                    leaf("ping", "offline:ping"),
                ])
                .enabled(false),
                MenuItem::submenu("Empty", []),
            ])
            .id("attach-sub")
            .menu("Session"),
        )
        .menu_item(leaf("Detach", "detach").menu("Session").after("attach-sub"))
        .menu_item(
            MenuItem::submenu("Dock Hosts", [
                leaf("local", "dock:local"),
            ])
            .dock(),
        );
    let _guard = MenuGuard::attach(&app.shared, true);
    assert!(app.handle().take_menu_errors().is_empty());

    // Firing by id and by unique nested title works; submenu titles and separators cannot fire.
    let mut state = Vec::new();
    fire(&app.shared, "prod:main").unwrap();
    fire(&app.shared, "api").unwrap();
    fire(&app.shared, "dock:local").unwrap();
    fire(&app.shared, "Detach").unwrap();
    drain(&app, &mut state);
    assert_eq!(state, ["prod:main", "staging:api", "dock:local", "detach"]);

    assert_eq!(fire(&app.shared, "Attach"), Err(MenuError::UnknownItem("Attach".into())));
    assert_eq!(fire(&app.shared, "attach-sub"), Err(MenuError::UnknownItem("attach-sub".into())));
    assert_eq!(fire(&app.shared, "Empty"), Err(MenuError::UnknownItem("Empty".into())));
    assert_eq!(fire(&app.shared, ""), Err(MenuError::UnknownItem(String::new())));

    // Ambiguous title across sibling submenus requires an explicit id.
    assert_eq!(fire(&app.shared, "main"), Err(MenuError::AmbiguousTitle("main".into())));
    fire(&app.shared, "staging:main").unwrap();
    drain(&app, &mut state);
    assert_eq!(state.last().map(String::as_str), Some("staging:main"));

    // Disabled leaf and any leaf under a disabled submenu (1 or 2 levels up) return Disabled.
    assert_eq!(fire(&app.shared, "prod:logs"), Err(MenuError::Disabled("prod:logs".into())));
    assert_eq!(fire(&app.shared, "offline:ping"), Err(MenuError::Disabled("offline:ping".into())));
    assert_eq!(fire(&app.shared, "ping"), Err(MenuError::Disabled("ping".into())));
    assert_eq!(
        fire(&app.shared, "offline:deep:unreachable"),
        Err(MenuError::Disabled("offline:deep:unreachable".into())),
    );
    assert_eq!(
        fire(&app.shared, "unreachable"),
        Err(MenuError::Disabled("unreachable".into())),
    );

    // Same submenu title under different parents is valid as long as leaf ids are unique.
    let handle = app.handle();
    handle.set_menu([
        MenuItem::submenu("Host A", [
            MenuItem::submenu("Windows", [leaf("0", "a:0")]),
        ]).menu("Session"),
        MenuItem::submenu("Host B", [
            MenuItem::submenu("Windows", [leaf("0", "b:0")]),
        ]).menu("Session"),
    ]).unwrap();
    fire(&app.shared, "b:0").unwrap();
    drain(&app, &mut state);
    assert_eq!(state.last().map(String::as_str), Some("b:0"));

    // Validation failures are atomic and leave the previous valid snapshot untouched.
    let bad_cases: Vec<Vec<MenuItem<Vec<String>>>> = vec![
        // Empty / whitespace submenu title
        vec![MenuItem::submenu("   ", [leaf("ok", "ok")]).menu("Session")],
        // Empty named menu title on submenu
        vec![MenuItem::submenu("Sub", [leaf("ok", "ok")]).menu("  ")],
        // Duplicate leaf id between top-level and nested submenu
        vec![leaf("Top", "dup"), MenuItem::submenu("Sub", [leaf("Nested", "dup")])],
        // Duplicate leaf id across two nested submenus
        vec![
            MenuItem::submenu("A", [leaf("One", "dup")]),
            MenuItem::submenu("B", [MenuItem::submenu("C", [leaf("Two", "dup")])]),
        ],
        // Empty leaf title inside submenu
        vec![MenuItem::submenu("Sub", [MenuItem::new("   ", |_, _| {}).id("ok")])],
        // Empty leaf id inside submenu
        vec![MenuItem::submenu("Sub", [MenuItem::new("Title", |_, _| {}).id("   ")])],
        // Nested item trying to override top-level menu or dock
        vec![MenuItem::submenu("Sub", [leaf("Bad", "bad").menu("File")])],
        vec![MenuItem::submenu("Sub", [leaf("Bad", "bad").dock()])],
        // Nested submenu trying to override top-level menu
        vec![MenuItem::submenu("Outer", [MenuItem::submenu("Inner", []).menu("File")])],
        // MenuItem::about inside a submenu
        vec![MenuItem::submenu("Sub", [MenuItem::about("About", |_, _| {})])],
    ];
    for bad in bad_cases {
        assert!(matches!(handle.set_menu(bad), Err(MenuError::Invalid(_))));
    }
    // Previous valid snapshot is still active after every rejected update.
    fire(&app.shared, "a:0").unwrap();
    drain(&app, &mut state);
    assert_eq!(state.last().map(String::as_str), Some("a:0"));
}

#[test]
fn submenu_replacement_and_disabling_cancel_stale_queued_actions() {
    let app = App::new(AppSpec::new("stale-sub", (100.0, 100.0), Vec::<String>::new, |_, _| el()));
    let make = |enabled: bool, tag: &'static str| {
        vec![MenuItem::submenu("Hosts", [
            MenuItem::with_action("prod", move |s: &mut Vec<String>, _| s.push(tag.into()))
                .id("host-prod"),
        ])
        .enabled(enabled)
        .menu("Session")]
    };
    let app = make(true, "v1").into_iter().fold(app, App::menu_item);
    let _guard = MenuGuard::attach(&app.shared, true);
    let handle = app.handle();

    // Queue a click on v1, then disable the enclosing submenu before drain: stale action drops.
    fire(&app.shared, "host-prod").unwrap();
    handle.set_menu(make(false, "v2")).unwrap();
    let mut state = Vec::new();
    drain(&app, &mut state);
    assert!(state.is_empty());

    // Re-enable with a fresh closure capturing "v3" and confirm the new closure runs.
    handle.set_menu(make(true, "v3")).unwrap();
    fire(&app.shared, "host-prod").unwrap();
    drain(&app, &mut state);
    assert_eq!(state, ["v3"]);
}

#[test]
fn large_dynamic_submenu_tree_validates_and_dispatches_exact_leaf() {
    let app = App::new(AppSpec::new("large-sub", (100.0, 100.0), Vec::<usize>::new, |_, _| el()));
    let groups: Vec<_> = (0..20)
        .map(|g| {
            let leaves = (0..10).map(move |i| {
                let value = g * 10 + i;
                MenuItem::with_action(format!("Session {i}"), move |s: &mut Vec<usize>, _| s.push(value))
                    .id(format!("g{g}-s{i}"))
            });
            MenuItem::submenu(format!("Host {g}"), leaves).menu("Session")
        })
        .collect();
    let app = groups.into_iter().fold(app, App::menu_item);
    let _guard = MenuGuard::attach(&app.shared, true);
    assert!(app.handle().take_menu_errors().is_empty());

    let mut state = Vec::new();
    fire(&app.shared, "g0-s0").unwrap();
    fire(&app.shared, "g13-s7").unwrap();
    fire(&app.shared, "g19-s9").unwrap();
    drain(&app, &mut state);
    assert_eq!(state, [0, 137, 199]);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn unsupported_native_platform_is_a_noop_but_headless_works() {
    let app = app().menu_item(item("quick"));
    let _guard = MenuGuard::attach(&app.shared, false);
    assert_eq!(app.handle().take_menu_errors(), [MenuError::Unsupported]);
    assert_eq!(app.handle().set_menu([item("other")]), Err(MenuError::Unsupported));
    assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions[0].meta.id, "quick");
}

#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn macos_native_submenus_build_fire_and_clear() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    // AppKit menus require the main thread (run with --test-threads=1 to execute this test).
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.setMainMenu(None);

    let hits: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink_hits = hits.clone();
    let sink: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |id: &str| {
        sink_hits.lock().unwrap().push(id.to_owned());
    });

    let items: Vec<MenuItem<()>> = vec![
        MenuItem::submenu("Attach", [
            MenuItem::submenu("prod", [
                MenuItem::with_action("main", |_, _| {}).id("prod:main").checked(true),
                MenuItem::separator(),
                MenuItem::with_action("logs", |_, _| {}).id("prod:logs").enabled(false),
            ]),
            MenuItem::submenu("offline", [
                MenuItem::with_action("unreachable", |_, _| {}).id("offline:unreachable"),
            ])
            .enabled(false),
            MenuItem::submenu("Empty", []),
        ])
        .id("attach-sub")
        .menu("Session"),
        MenuItem::with_action("Detach", |_, _| {})
            .id("detach")
            .menu("Session")
            .after("attach-sub"),
        MenuItem::submenu("Disabled Top", [
            MenuItem::with_action("Nope", |_, _| {}).id("disabled-top:nope"),
        ])
        .enabled(false)
        .menu("Session"),
    ];

    let mut actions = Vec::new();
    let descriptions: Vec<_> = items.into_iter().map(|i| i.split(&mut actions)).collect();
    validate(&descriptions).unwrap();

    let mut native = macos::Native::new().unwrap();
    native.set(&descriptions, sink).unwrap();

    // SAFETY: main-thread inspection of menus we own, without opening a window.
    unsafe {
        let root = app.mainMenu().expect("main menu installed");
        let session_top = (0..root.numberOfItems())
            .filter_map(|i| root.itemAtIndex(i))
            .find(|i| default_macos::name(i) == "Session")
            .expect("Session top-level menu");
        let session = session_top.submenu().expect("Session submenu");
        // Update parent menu as AppKit does before tracking, to verify disabled submenus stay disabled.
        session.update();
        assert_eq!(session.numberOfItems(), 3);

        let attach_item = session.itemAtIndex(0).unwrap();
        let detach_item = session.itemAtIndex(1).unwrap();
        let disabled_top_item = session.itemAtIndex(2).unwrap();
        assert_eq!(attach_item.title().to_string(), "Attach");
        assert!(attach_item.isEnabled());
        assert_eq!(detach_item.title().to_string(), "Detach");
        assert_eq!(disabled_top_item.title().to_string(), "Disabled Top");
        assert!(!disabled_top_item.isEnabled(), "disabled top-level submenu must stay disabled after menu.update()");

        let attach_menu = attach_item.submenu().expect("Attach NSMenu");
        attach_menu.update();
        assert_eq!(attach_menu.numberOfItems(), 3);
        let prod_item = attach_menu.itemAtIndex(0).unwrap();
        let offline_item = attach_menu.itemAtIndex(1).unwrap();
        let empty_item = attach_menu.itemAtIndex(2).unwrap();
        assert_eq!(prod_item.title().to_string(), "prod");
        assert!(prod_item.isEnabled());
        assert_eq!(offline_item.title().to_string(), "offline");
        assert!(!offline_item.isEnabled(), "disabled nested submenu must stay disabled");
        assert_eq!(empty_item.title().to_string(), "Empty");
        assert_eq!(empty_item.submenu().unwrap().numberOfItems(), 0);

        // Deeply nested leaf items: checked state, separator, disabled leaf, and firing via AppKit.
        let prod_menu = prod_item.submenu().expect("prod NSMenu");
        assert_eq!(prod_menu.numberOfItems(), 3);
        let main_item = prod_menu.itemAtIndex(0).unwrap();
        let sep_item = prod_menu.itemAtIndex(1).unwrap();
        let logs_item = prod_menu.itemAtIndex(2).unwrap();
        assert_eq!(main_item.title().to_string(), "main");
        assert_eq!(main_item.state(), 1);
        assert!(main_item.isEnabled());
        assert!(sep_item.isSeparatorItem());
        assert_eq!(logs_item.title().to_string(), "logs");
        assert!(!logs_item.isEnabled());

        prod_menu.performActionForItemAtIndex(0);
        prod_menu.performActionForItemAtIndex(2);
        // Child inside disabled submenu must not fire even if invoked directly.
        let offline_menu = offline_item.submenu().expect("offline NSMenu");
        let unreachable_item = offline_menu.itemAtIndex(0).unwrap();
        assert!(!unreachable_item.isEnabled());
        offline_menu.performActionForItemAtIndex(0);
        assert_eq!(*hits.lock().unwrap(), ["prod:main"]);

        // Dropping Native clears nested targets recursively and removes the empty Session menu.
        drop(native);
        assert!(main_item.target().is_none(), "nested leaf target must be cleared on teardown");
        assert!(main_item.action().is_none(), "nested leaf action must be cleared on teardown");
        assert!(unreachable_item.target().is_none(), "disabled nested leaf target must be cleared");
        let still_has_session = (0..root.numberOfItems())
            .filter_map(|i| root.itemAtIndex(i))
            .any(|i| default_macos::name(&i) == "Session");
        assert!(!still_has_session, "empty custom top-level menu should be removed on teardown");
        app.setMainMenu(None);
    }
}
