#!/bin/sh
# Records a motion trace of every element (--trace-all) for each scenario below, at 120 fps on
# the virtual clock, plus the final element tree. Then scripts/motion-audit/analyze.py reads them.
# Run from the repo root: scripts/motion-audit/run.sh [out-dir] (default target/motion-audit).
set -e
O=${1:-target/motion-audit}
mkdir -p "$O"
cargo build --release -p todo -p settings -p kit-gallery -p odometer
cargo build --release -p rune-kit --example shell
cargo build --release -p rune --example growth_reorder --example keep_stuck --example sticky --example scroll_keys --example scroll_anchor
B=${CARGO_TARGET_DIR:-target}/release
SH=$B/examples/shell
GR=$B/examples/growth_reorder
KS=$B/examples/keep_stuck
S=scripts/motion-audit/scenarios
FPS=120

# name | seconds | command (trace args appended)
run() {
  name=$1; secs=$2; shift 2
  frames=$(python3 -c "print(int($secs * $FPS))")
  echo "== $name ($frames frames)"
  "$@" --fps $FPS --frames "$frames" --trace-all --trace "$O/$name.jsonl" --tree "$O/$name-end.json"
}

run todo-add-remove   4.4 $B/todo --script $S/todo-add-remove.json
run settings-toggle-retarget 1.5 $B/settings --script examples/settings/scripts/toggle-retarget.json
run settings-segmented-retarget 1.5 $B/settings --script examples/settings/scripts/segmented-retarget.json
run settings-slider   3.0 $B/settings --script examples/settings/scripts/slider.json
run settings-hover    3.0 $B/settings --script examples/settings/scripts/hover.json
run gallery-combobox  3.0 $B/kit-gallery --scene rune-combobox--searchable --script examples/kit-gallery/scripts/combobox.json
run gallery-combobox-animated 3.0 $B/kit-gallery --scene rune-combobox--animated-filter --script examples/kit-gallery/scripts/combobox-animated-filter.json
run gallery-modal     5.5 $B/kit-gallery --scene motion-focus-modal --script examples/kit-gallery/scripts/motion-focus-modal.json
run gallery-stagger-list 3.0 $B/kit-gallery --scene motion-stagger-list --script examples/kit-gallery/scripts/motion-stagger-list.json
run odometer          5.4 $B/odometer --script $S/odometer.json
run shell-idle        2.0 $SH
run shell-toggle      3.0 $SH --script $S/shell-toggle.json
run shell-peek        5.0 $SH --script $S/shell-peek.json
run shell-resize      3.6 $SH --script $S/shell-resize.json
run shell-reorder     3.6 $SH --script $S/shell-reorder.json
run shell-virtual-peek 5.0 $SH virtual --script $S/shell-peek.json
run shell-virtual-reorder 3.6 $SH virtual --script $S/shell-reorder-virtual.json
run shell-window-resize 3.0 $SH --script $S/shell-window-resize.json
run shell-virtual-window-resize 3.0 $SH virtual --script $S/shell-window-resize.json
run growth-reorder    4.2 $GR --script $S/growth-reorder.json
run growth-reorder-virtual 4.0 $GR virtual --script $S/growth-reorder-virtual.json
run keep-stuck        3.2 $KS --script $S/keep-stuck.json
run growth-reorder-grow 3.5 $GR steer --script $S/growth-reorder-grow.json
run growth-reorder-gap 4.7 $GR steer --script $S/growth-reorder-gap.json
run growth-reorder-flight 2.6 $GR steer --script $S/growth-reorder-flight.json
run sticky-scroll     1.6 $B/examples/sticky --script crates/rune/examples/sticky.json
run scroll-keys       3.2 $B/examples/scroll_keys --script $S/scroll-keys.json
run scroll-anchor     3.4 $B/examples/scroll_anchor --script $S/scroll-anchor.json
ls "$O"
