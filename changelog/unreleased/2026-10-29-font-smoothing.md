- **`FontSmoothing` picks how text is rasterised: `On` (the macOS default, CoreText with font
  smoothing on) or `Off` (the default on Linux, Windows and the web: CoreText with font
  smoothing off, as `-webkit-font-smoothing: antialiased` draws).** Both run CoreGraphics'
  glyph pipeline; `On` adds font smoothing's dilation. Set it app-wide with
  `App::font_smoothing` or `WindowSpec::font_smoothing`, or while running with
  `Tx::set_font_smoothing`; each mode keeps its own glyphs in the atlas. Blending is separate
  (`srgb_blend`, on by default). `Runtime::set_font_smoothing` and `Runtime::font_smoothing` do
  the same for a runtime an app draws itself.
- **Breaking: `FontSmoothing::Calibrated` (Rune's previous rasteriser) is removed, and `MacOS` is
  renamed `On`.** Its quarter-pixel pens, 0.1 pt outline
  grow and luminance/scale coverage curve are gone, with `rune::text::glyph_coverage_exponent`.
  `TextSystem::apple_pipeline` returns an `ApplePipeline` and `TextSystem::set_apple_pipeline`
  takes one (no `Option`). The `glyphs_for_layout` and `glyphs_in` `scale` arguments are now
  unused.
