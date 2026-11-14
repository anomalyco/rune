#!/usr/bin/env bash
# Re-makes vendor/accesskit_macos: the accesskit_macos crate exactly as published on crates.io at
# <version>, without its integration tests, plus vendor/accesskit_macos.patch (headings as AppKit's
# AXHeading with their level as the value, which the published adapter reports as a "Heading" role
# VoiceOver doesn't know, so VoiceOver's rotor and VO-Command-H found no heading). Run it when
# AccessKit is bumped, then build: a patch that no longer applies fails here, and the change goes
# back in by hand. If upstream AccessKit takes the change, delete vendor/accesskit_macos and its
# `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-accesskit-macos.sh <version>     e.g. scripts/vendor-accesskit-macos.sh 0.27.2
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-accesskit-macos.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/accesskit_macos/accesskit_macos-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/accesskit_macos
mv "$tmp/accesskit_macos-$version" vendor/accesskit_macos
rm -rf vendor/accesskit_macos/tests
git apply --directory=vendor/accesskit_macos vendor/accesskit_macos.patch
echo "vendor/accesskit_macos: accesskit_macos $version + vendor/accesskit_macos.patch"
