#!/bin/sh
# A poll delivers a pending row; unchanged window, resizing, and reading above the end.
exec scripts/motion-audit/gate live-tail -- -p rune --lib live_tail_motion_gate
