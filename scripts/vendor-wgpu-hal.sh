#!/usr/bin/env bash
# Re-makes vendor/wgpu-hal: the wgpu-hal crate exactly as published on crates.io at <version>, plus
# vendor/wgpu-hal.patch (`dx12::Instance::expose_adapter`, which `gpu/pick.rs` uses to hand wgpu the one
# adapter it picked through DXGI; FXC loaded on first use; `Surface::set_present_rects`, partial presents). Run it when wgpu is bumped, then build: a patch
# that no longer applies fails here, and the function goes back in by hand. If upstream wgpu takes
# the function, delete vendor/ and the `[patch.crates-io]` entries instead.
# Usage: scripts/vendor-wgpu-hal.sh <version>     e.g. scripts/vendor-wgpu-hal.sh 30.0.1
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-wgpu-hal.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/wgpu-hal/wgpu-hal-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/wgpu-hal
mv "$tmp/wgpu-hal-$version" vendor/wgpu-hal
git apply --directory=vendor/wgpu-hal vendor/wgpu-hal.patch
echo "vendor/wgpu-hal: wgpu-hal $version + vendor/wgpu-hal.patch"
