# Third-Party Notices

Rune incorporates the third-party assets and libraries listed below. Original copyright notices and license texts are preserved for attribution and redistribution compliance. Bundled fonts under `crates/rune/assets/`, `themes/`, and `www/public/fonts/` and vendored packages under `vendor/` and `crates/rune-highlights/vendor/` ship their own license files alongside those assets.

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

---

## 3. Chromium Windows Cursor Bitmaps (`crates/rune/src/input/cursor/*.png`)

18 PNG cursor images derived from Chromium's Windows cursor resources (`ui/resources/cursors/*.cur` in `https://chromium.googlesource.com/chromium/src`, declared in `ui/resources/ui_unscaled_resources.grd`). Each 32×32 1-bit `.cur` bitmap was converted to a 64×64 RGBA PNG by doubling each pixel (2× nearest-neighbor) while preserving its original hot spot in `crates/rune/src/input/cursor.rs`:

- **`Cursor::Cell` (Windows)** — `cell.png` from `ui/resources/cursors/cell.cur`, hot spot `[7, 7]` (added in #582)
- **Middle-click autoscroll pan cursors (Windows and Linux, `crates/rune/src/runtime/input/autoscroll.rs`)** — 11 files (`pan_east.png`, `pan_west.png`, `pan_north.png`, `pan_south.png`, `pan_north_east.png`, `pan_north_west.png`, `pan_south_east.png`, `pan_south_west.png`, `pan_middle.png`, `pan_middle_horizontal.png`, `pan_middle_vertical.png`) from `ui/resources/cursors/pan_*.cur`, hot spot `[16, 16]` (added in #874)
- **`Cursor::Grab`, `Cursor::Grabbing`, `Cursor::ZoomIn`, `Cursor::ZoomOut` (Windows)** — `grab.png` (`hand_grab.cur`, hot spot `[13, 13]`), `grabbing.png` (`hand_grabbing.cur`, hot spot `[13, 13]`), `zoom_in.png` (`zoom_in.cur`, hot spot `[6, 6]`), `zoom_out.png` (`zoom_out.cur`, hot spot `[6, 6]`) (added in #2123)
- **`Cursor::Alias`, `Cursor::Copy` (Windows)** — `alias.png` (`aliasb.cur`, hot spot `[7, 4]`), `copy.png` (`copy.cur`, hot spot `[7, 4]`) (added in #2379)

`ui/resources/cursors/` carries no separate license file; these resources are covered by Chromium's root `LICENSE` (**BSD 3-Clause License**):

```text
Copyright 2015 The Chromium Authors

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
