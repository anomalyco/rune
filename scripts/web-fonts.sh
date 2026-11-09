#!/usr/bin/env bash
# Regenerate crates/rune/assets/web/: the bundled Inter faces without TrueType hinting or glyph
# names, for the web, whose rasterizer reads neither. Every glyph, mapping, layout table and name
# stays. IBM Plex Mono ships as is: its licence reserves the name "Plex" for unmodified fonts.
# web/first/ holds the same faces cut to Latin, Greek and Cyrillic text, which a web app embeds
# for its first frame; it fetches the full faces after that. Needs HarfBuzz's hb-subset (brew
# install harfbuzz). Check with `cargo test -p rune --lib default_fonts full_faces`.
set -euo pipefail
cd "$(dirname "$0")/../crates/rune/assets"
mkdir -p web/first
# Basic Latin, Latin-1, Latin Extended-A, Romanian's comma-below letters, spacing accents,
# Greek, Cyrillic with Ukrainian's Ghe with upturn, general punctuation, the euro and trade mark
# signs, arrows and the Mac's key symbols.
first='U+0020-007E,U+00A0-017F,U+0192,U+0218-021B,U+02C6-02DD,U+0370-03FF,U+0400-045F,U+0490-0491,U+2000-206F,U+20AC,U+2122,U+2190-2193,U+21A9,U+21E7,U+2212,U+2303,U+2318,U+2325,U+232B,U+238B,U+23CE,U+FFFD'
for font in Inter-Regular Inter-Medium Inter-SemiBold; do
    hb-subset "$font.ttf" --unicodes='*' --glyphs='*' --retain-gids --notdef-outline \
        --layout-features='*' --layout-scripts='*' --name-IDs='*' --name-languages='*' \
        --no-prune-unicode-ranges --no-hinting -o "web/$font.ttf"
    hb-subset "$font.ttf" --unicodes="$first" --notdef-outline \
        --layout-features='*' --layout-scripts='*' --name-IDs='*' --name-languages='*' \
        --no-hinting -o "web/first/$font.ttf"
    echo "web/$font.ttf: $(wc -c < "$font.ttf" | tr -d ' ') -> $(wc -c < "web/$font.ttf" | tr -d ' ') bytes, first $(wc -c < "web/first/$font.ttf" | tr -d ' ')"
done
