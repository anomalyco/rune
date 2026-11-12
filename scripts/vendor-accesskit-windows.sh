#!/usr/bin/env bash
# Re-makes vendor/accesskit_windows: the accesskit_windows crate exactly as published on crates.io at
# <version>, plus vendor/accesskit_windows.patch (UI Automation's HeadingLevel, LandmarkType and
# LocalizedLandmarkType for headings and landmarks, which the published adapter doesn't report, so
# Narrator's and NVDA's heading and landmark navigation found none). Run it when AccessKit is
# bumped, then build: a patch that no longer applies fails here, and the change goes back in by
# hand. If upstream AccessKit takes the change, delete vendor/accesskit_windows and its
# `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-accesskit-windows.sh <version>     e.g. scripts/vendor-accesskit-windows.sh 0.35.2
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-accesskit-windows.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/accesskit_windows/accesskit_windows-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/accesskit_windows
mv "$tmp/accesskit_windows-$version" vendor/accesskit_windows
git apply --directory=vendor/accesskit_windows vendor/accesskit_windows.patch
echo "vendor/accesskit_windows: accesskit_windows $version + vendor/accesskit_windows.patch"