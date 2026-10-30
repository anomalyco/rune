- **Theme colours authored for sRGB blending are painted as authored now that windows blend in
  sRGB.** A theme with `"blend": {"mode": "srgb"}`, the OpenCode theme's translucent tokens and
  shadows, `Kit::alpha`, `Kit::on` and `Kit::from_palette`'s borders and tints re-solve alpha
  only in a window that blends in linear light (`srgb_blend(false)`), so fills, selected rows,
  disabled controls and status banners look as they did before the sRGB default instead of
  double-compensated. Colours resolve for the blend mode of the window building the view
  (`rune::theme::blends_srgb`, `rune::theme::for_blend`, `rune::theme::with_blend`), a live
  `Tx::set_srgb_blend` re-resolves them (memoised rows included), and
  `rune_theme_opencode::kit` returns the kit for the current mode, so get it in the view.
