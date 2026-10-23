// Consumers (Rooms, Hex, Deadbolt) spell every `AppSpec` field; the dev feature must not add one.
use rune_dev_panel::{State, view};

#[test]
fn exhaustive_app_spec_literals_compile_with_the_dev_panel() {
    let spec = rune::AppSpec {
        title: "literal",
        size: (100.0, 100.0),
        min_size: None,
        init: State::default,
        view,
        scenes: &[],
        fonts: &[],
        families: ("Inter", "Inter"),
        setup: None,
        poll: None,
        on_key: None,
    };
    let app = rune::App::new(spec);
    #[cfg(all(feature = "dev", debug_assertions))]
    let app = app.dev_panel(rune_dev_panel::config());
    drop(app);
}
