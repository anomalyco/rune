#!/bin/sh
# Matched flights with own El::rotate and El::live_rotation on source, target, both and mid-flight re-match.
exec scripts/motion-audit/gate matched-rotation -- -p rune --lib matched_rotation_motion_gate
