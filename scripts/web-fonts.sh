#!/usr/bin/env bash
# Regenerate crates/rune/assets/web/: the bundled Inter faces without TrueType hinting or glyph
# names, for the web, whose rasterizer reads neither. Every glyph, mapping, layout table and name
# stays. IBM Plex Mono ships as is: its licence reserves the name "Plex" for unmodified fonts.
# Needs HarfBuzz's hb-subset (brew install harfbuzz). Check with
# `cargo test -p rune --lib default_fonts`.
set -euo pipefail
cd "$(dirname "$0")/../crates/rune/assets"
mkdir -p web
for font in Inter-Regular Inter-Medium Inter-SemiBold; do
    hb-subset "$font.ttf" --unicodes='*' --glyphs='*' --retain-gids --notdef-outline \
        --layout-features='*' --layout-scripts='*' --name-IDs='*' --name-languages='*' \
        --no-prune-unicode-ranges --no-hinting -o "web/$font.ttf"
    echo "web/$font.ttf: $(wc -c < "$font.ttf" | tr -d ' ') -> $(wc -c < "web/$font.ttf" | tr -d ' ') bytes"
done
