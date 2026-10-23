fn main() {
    let app = rune::App::new(rune_dev_panel::spec());
    let app = if std::env::args().any(|arg| arg == "--windowless") { app.windowless().accessory() } else { app };
    #[cfg(all(feature = "dev", debug_assertions))]
    let app = app.dev_panel(rune_dev_panel::config());
    app.run();
}
