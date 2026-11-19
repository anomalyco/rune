#!/usr/bin/env bash
# Re-makes vendor/sctk-adwaita: the sctk-adwaita crate exactly as published on crates.io at
# <version>, plus vendor/sctk-adwaita.patch (the visible one-point border follows the window's
# rounded bottom corners, as the headerbar above them already rounds the top ones, instead of
# drawing a square L there; and `AdwaitaFrame::content_corner_radius` says what radius the content
# leaves its own bottom corners to, for the app to clear outside it). Run it when sctk-adwaita is
# bumped, then build: a patch that no longer applies fails here, and the change goes back in by
# hand. If upstream sctk-adwaita takes the change, delete vendor/sctk-adwaita and its
# `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-sctk-adwaita.sh <version>     e.g. scripts/vendor-sctk-adwaita.sh 0.10.1
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-sctk-adwaita.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/sctk-adwaita/sctk-adwaita-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/sctk-adwaita
mv "$tmp/sctk-adwaita-$version" vendor/sctk-adwaita
git apply --directory=vendor/sctk-adwaita vendor/sctk-adwaita.patch
echo "vendor/sctk-adwaita: sctk-adwaita $version + vendor/sctk-adwaita.patch"
