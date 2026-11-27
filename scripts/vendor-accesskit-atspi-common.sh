#!/usr/bin/env bash
# Re-makes vendor/accesskit_atspi_common: the accesskit_atspi_common crate exactly as published on
# crates.io at <version>, plus vendor/accesskit_atspi_common.patch (reads an item node's own
# size_of_set on item roles before falling back to size_of_set_from_container, so flat trees with
# per-level counts report each level's setsize to AT-SPI without leaking container sizes onto
# containers; exposes aria_current as the AT-SPI2 current object attribute; and exposes custom
# accessibility actions on AT-SPI's Action interface). Run it when AccessKit is bumped, then build:
# a patch that no longer applies fails
# here, and the change goes back in by hand. If upstream AccessKit takes the change, delete
# vendor/accesskit_atspi_common and its `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-accesskit-atspi-common.sh <version>     e.g. scripts/vendor-accesskit-atspi-common.sh 0.21.1
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-accesskit-atspi-common.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/accesskit_atspi_common/accesskit_atspi_common-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/accesskit_atspi_common
mv "$tmp/accesskit_atspi_common-$version" vendor/accesskit_atspi_common
git apply --directory=vendor/accesskit_atspi_common vendor/accesskit_atspi_common.patch
echo "vendor/accesskit_atspi_common: accesskit_atspi_common $version + vendor/accesskit_atspi_common.patch"
