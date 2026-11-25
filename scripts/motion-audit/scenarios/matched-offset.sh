#!/bin/sh
# Matched flights with own El::offset and El::live_offset on source, target, both and mid-flight re-match.
exec scripts/motion-audit/gate matched-offset -- -p rune --lib matched_offset_motion_gate
