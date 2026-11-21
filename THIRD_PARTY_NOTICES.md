# Third-Party Notices

Rune incorporates the third-party assets and libraries listed below. Original copyright notices and license texts are preserved for attribution and redistribution compliance.

---

## 1. KaTeX Math Fonts (`crates/rune-kit/assets/math/katex-fonts.pack.deflate`, feature `rune-kit/math`)

Bundled from `ratex-katex-fonts 0.1.16` (origin: KaTeX, `https://github.com/KaTeX/KaTeX`). Verbatim notices and the full SIL Open Font License 1.1 text are shipped in `crates/rune-kit/assets/math/FONT_NOTICE.txt` and `crates/rune-kit/assets/math/OFL.txt`.

- **KaTeX Fonts** (20 TrueType faces with Reserved Font Names `KaTeX_*`: `KaTeX_AMS-Regular`, `KaTeX_Caligraphic-Bold`, `KaTeX_Caligraphic-Regular`, `KaTeX_Fraktur-Bold`, `KaTeX_Fraktur-Regular`, `KaTeX_Main-Bold`, `KaTeX_Main-BoldItalic`, `KaTeX_Main-Italic`, `KaTeX_Main-Regular`, `KaTeX_Math-BoldItalic`, `KaTeX_Math-Italic`, `KaTeX_SansSerif-Bold`, `KaTeX_SansSerif-Italic`, `KaTeX_SansSerif-Regular`, `KaTeX_Script-Regular`, `KaTeX_Size1-Regular`, `KaTeX_Size2-Regular`, `KaTeX_Size3-Regular`, `KaTeX_Size4-Regular`, `KaTeX_Typewriter-Regular`)
  - License: **SIL Open Font License, Version 1.1** (`crates/rune-kit/assets/math/OFL.txt`)
  - Copyright (c) 2009–2010 Design Science, Inc.
  - Copyright (c) 2014–2018 Khan Academy (KaTeX)

---

## 2. Sandboxed LaTeX Display-Math Typesetter (`crates/rune-kit/assets/math/ratex-math.wasm.deflate`, feature `rune-kit/math`)

Built reproducibly from `tools/ratex-math-wasm/` (`scripts/build-math-wasm.sh`):

- **RaTeX** (`ratex-parser`, `ratex-layout`, `ratex-types`, `ratex-font`, `ratex-lexer` `0.1.16`, `https://github.com/erweixin/RaTeX`) — **MIT License**
- **unicode-normalization** (`0.1.26`, compiled inside `ratex-math.wasm`, `https://github.com/unicode-rs/unicode-normalization`) — **(MIT OR Apache-2.0) AND Unicode-3.0**
- **wasmi** (`wasmi`, `wasmi_core`, `wasmi_ir`, `wasmi_collections` `1.1.0`, native host interpreter, `https://github.com/wasmi-labs/wasmi`) — **MIT OR Apache-2.0**
