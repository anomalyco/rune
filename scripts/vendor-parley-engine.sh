#!/usr/bin/env bash
# Re-makes vendor/parley_engine: the parley_engine crate exactly as published on crates.io at
# <version>, plus vendor/parley_engine.patch (UAX #9 rule L1 for a paragraph's trailing
# whitespace, isolate initiators and PDIs, which parley_engine leaves at their resolved levels:
# with it, Unicode's BidiTest.txt passes 770,241 of 770,241, without it 644,283), and three caches
# of work shaping redid from the same inputs: `Analyzer` keeps the line and word segmenters it made
# (`analysis.rs` `Segmenters`; making one loads the ICU dictionaries), `Shaper` keeps each run's
# font metrics per font instance and size (`run_metrics_cache`), and `FontMetrics::from_font_instance`
# keeps a style's per thread (64 entries). Two hooks for a host without ICU's dictionaries or the
# fonts a script needs (Rune's web build): `dictionary::set_dictionary_segmenter` (the words, and
# Thai, Lao, Khmer and Burmese line breaks, of runs that need a dictionary) and
# `platform::set_platform_face` (a face whose runs the host measures and draws, not HarfRust;
# `push_run` takes glyph infos and positions instead of a `GlyphBuffer` for it). Run it when
# parley is bumped, then build: a patch that no longer applies fails here, and the change goes
# back in by hand. If upstream parley takes the change, delete vendor/parley_engine and its
# `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-parley-engine.sh <version>     e.g. scripts/vendor-parley-engine.sh 0.12.0
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-parley-engine.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/parley_engine/parley_engine-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/parley_engine
mv "$tmp/parley_engine-$version" vendor/parley_engine
git apply --directory=vendor/parley_engine vendor/parley_engine.patch
echo "vendor/parley_engine: parley_engine $version + vendor/parley_engine.patch"