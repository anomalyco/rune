#[test]
fn wayland_csd_title_feature_is_default_on_and_opt_out_keeps_notitle_frames() {
    let rune_manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read crates/rune/Cargo.toml");

    assert!(
        rune_manifest.contains(r#"wayland-csd-title = ["winit/wayland-csd-adwaita"]"#),
        "rune must define wayland-csd-title forwarding to winit/wayland-csd-adwaita"
    );

    let default_line = rune_manifest
        .lines()
        .find(|line| line.trim_start().starts_with("default = ["))
        .expect("crates/rune/Cargo.toml must define default features");
    assert!(
        default_line.contains(r#""wayland-csd-title""#),
        "wayland-csd-title must be enabled by default so plain Wayland CSD windows show titles"
    );

    let winit_dep_line = rune_manifest
        .lines()
        .find(|line| line.trim_start().starts_with("winit = {"))
        .expect("crates/rune/Cargo.toml must declare winit dependency");
    assert!(
        winit_dep_line.contains(r#""wayland-csd-adwaita-notitle""#),
        "base winit dependency must keep wayland-csd-adwaita-notitle for --no-default-features builds"
    );
    assert!(
        !winit_dep_line.contains(r#""wayland-csd-adwaita""#),
        "base winit dependency must not hardcode wayland-csd-adwaita so default-features = false can omit ab_glyph"
    );
}

#[test]
fn sctk_adwaita_early_config_prefetches_ab_glyph_title_font() {
    let config_rs = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/sctk-adwaita/src/config.rs"
    ))
    .expect("read vendor/sctk-adwaita/src/config.rs");
    let ab_glyph_rs = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/sctk-adwaita/src/title/ab_glyph_renderer.rs"
    ))
    .expect("read vendor/sctk-adwaita/src/title/ab_glyph_renderer.rs");

    assert!(
        config_rs.contains("title_font: Mutex::new(start(crate::title::read_title_font))"),
        "read_system_config_early must start the ab_glyph title font lookup off the main thread"
    );
    assert!(
        config_rs.contains("pub(crate) fn get_title_font()"),
        "config.rs must expose get_title_font to consume the early lookup once"
    );
    assert!(
        ab_glyph_rs.contains("crate::config::get_title_font()"),
        "AbGlyphTitleText::new must read the title font through crate::config::get_title_font()"
    );
}
