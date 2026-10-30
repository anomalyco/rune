#!/usr/bin/env python3
"""Checks analyze.py on small synthetic traces. Run: python3 scripts/motion-audit/test_analyze.py"""

import json
import math
import os
import sys
import tempfile

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(__file__))
import analyze  # noqa: E402


def trace(states):
    """A 120 Hz trace of one 10×10 element; `states` holds (x, opacity, visible width) per frame."""
    lines = [json.dumps({"width": 200, "height": 100, "hz": 120})]
    for k, (x, op, vis_w) in enumerate(states):
        fr = {"e": [[1, x, 40, 10, 10, op, 0, 0, max(x, 20), 40, vis_w, 10]], "quality": {"animating": True}}
        if k == 0:
            fr["m"] = [{"id": 1, "nm": "label/sheen"}]
        lines.append(json.dumps(fr))
    f = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
    f.write("\n".join(lines) + "\n")
    f.close()
    return f.name


def jumps(states):
    path = trace(states)
    try:
        return [f for f in analyze.analyze(path)["findings"] if f["kind"] in ("jump", "step")]
    finally:
        os.unlink(path)


def sheen(wrap_from, wrap_to):
    """A loop sliding right 0.5 pt a frame, then wrapping back to its start."""
    before = [(86 - 0.5 * (9 - i), *wrap_from) for i in range(10)]
    after = [(15 + 0.5 * i, *wrap_to) for i in range(10)]
    return before + after


def scene(frames, flags=None):
    """A 120 Hz trace of named elements; each frame maps name -> (x, y, w, h, opacity), and a
    name missing from a frame is gone. A name's parent is its longest named prefix."""
    names = sorted({nm for fr in frames for nm in fr})
    ids = {nm: i + 1 for i, nm in enumerate(names)}
    def parent(nm):
        cut = [nm[:j] for j in range(len(nm)) if nm[j] == "/" and nm[:j] in ids]
        return ids[cut[-1]] if cut else None
    lines = [json.dumps({"width": 400, "height": 400, "hz": 120})]
    alive, meta = set(), set()
    for fr in frames:
        out = {"e": [], "quality": {"animating": True}, "m": [], "g": []}
        for nm, (x, y, w, h, op) in fr.items():
            if nm not in meta:
                meta.add(nm)
                out["m"].append({"id": ids[nm], "nm": nm, "pv": parent(nm)})
            out["e"].append([ids[nm], x, y, w, h, op, 0, (flags or {}).get(nm, 0), x, y, w, h])
            alive.add(nm)
        for nm in sorted(alive - set(fr)):
            out["g"].append(ids[nm])
            alive.discard(nm)
        lines.append(json.dumps(out))
    f = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
    f.write("\n".join(lines) + "\n")
    f.close()
    return f.name


def kinds(frames, events=None, flags=None):
    path = scene(frames, flags)
    try:
        return [f["kind"] for f in analyze.analyze(path, {"events": events} if events else None)["findings"]]
    finally:
        os.unlink(path)


def ease(a, b, n):
    """n frames easing from a to b (no overshoot)."""
    return [a + (b - a) * (1 - (1 - (i + 1) / n) ** 3) for i in range(n)]


def hold(fr, n):
    return [dict(fr) for _ in range(n)]


def main():
    # instant: a key whose answer fades in over 20 frames isn't a cut.
    still = {"root": (0, 0, 400, 400, 1.0)}
    key = [{"t": 0.1, "type": "key", "name": "Escape", "expect": "instant"}]
    fade = hold(still, 12) + [{**still, "root/panel": (10, 10, 100, 100, op)} for op in ease(0, 1, 20)] + hold({**still, "root/panel": (10, 10, 100, 100, 1)}, 20)
    assert "not-instant" in kinds(fade, key), "a fade answering an instant input"
    cut = hold(still, 12) + hold({**still, "root/panel": (10, 10, 100, 100, 1)}, 40)
    assert "not-instant" not in kinds(cut, key) and "pop-in" not in kinds(cut, key), "a cut is the answer, not a pop"

    # follow: a drag that tracks the smoothstepped hand passes; one that trails it lags.
    drag = [{"t": 0.1, "type": "drag", "key": "handle", "dx": 120, "dur": 0.5, "expect": "follow", "element": "root/handle"}]
    def hand(k):
        f = max(0, min(60, math.floor((k / 120 - 0.1) / 0.5 * 60 + 1e-6))) / 60
        return 120 * f * f * (3 - 2 * f)
    track = [{**still, "root/handle": (10 + hand(k), 10, 20, 20, 1)} for k in range(100)]
    assert "lags-hand" not in kinds(track, drag), "a handle on the hand"
    lag, x = [], 10.0
    for k in range(100):
        x += (10 + hand(k) - x) * 0.3
        lag.append({**still, "root/handle": (x, 10, 20, 20, 1)})
    assert "lags-hand" in kinds(lag, drag), "a handle on a spring behind the hand"

    # one monotonic path: a wind-up the wrong way, and an L-shaped route.
    at = lambda x, y: {**still, "root/card": (x, y, 40, 40, 1)}
    windup = hold(at(100, 100), 5) + [at(x, 100) for x in ease(100, 94, 4) + ease(94, 200, 16)] + hold(at(200, 100), 10)
    assert "reversal" in kinds(windup), "a wind-up before the move"
    straight = hold(at(100, 100), 5) + [at(x, 100) for x in ease(100, 200, 20)] + hold(at(200, 100), 10)
    assert not {"reversal", "dog-leg", "overshoot"} & set(kinds(straight)), "a straight eased move"
    ell = hold(at(100, 100), 5) + [at(x, 100) for x in ease(100, 200, 10)] + [at(200, y) for y in ease(100, 200, 10)] + hold(at(200, 200), 10)
    assert "dog-leg" in kinds(ell), "across, then down"
    # a drift out and back that rests a frame where it turns is still one motion.
    drift = hold(at(100, 100), 5) + [at(x, 100) for x in ease(100, 97.5, 6)] + hold(at(97.5, 100), 2) + [at(x, 100) for x in ease(97.5, 100, 12)] + hold(at(100, 100), 10)
    assert "reversal" in kinds(drift), "out and back with a rest at the turn"

    # one steady speed: a retarget that drops part of its speed for a frame stalls; one that
    # keeps its speed and only pulls harder toward a farther target doesn't.
    xs = ease(100, 200, 20)
    slow = xs[7] + 0.3 * (xs[8] - xs[7])
    dip = xs[:8] + [slow] + [slow + v for v in ease(0, 230 - slow, 24)]
    assert "stall" in kinds(hold(at(100, 100), 5) + [at(x, 100) for x in dip] + hold(at(dip[-1], 100), 10)), "a re-aim that lost its speed for a frame"
    def spring(x, v, target, n, k=320.0, c=36.0, dt=1 / 120):
        out = []
        for _ in range(n):
            v += (k * (target - x) - c * v) * dt
            x += v * dt
            out.append(x)
        return out, v
    first, v = spring(100.0, 0.0, 200.0, 10)
    retarget, _ = spring(first[-1], v, 240.0, 60)
    assert "stall" not in kinds(hold(at(100, 100), 5) + [at(x, 100) for x in first + retarget] + hold(at(retarget[-1], 100), 10)), "a retarget at its own speed"
    assert "stall" not in kinds(straight), "a straight eased move"

    # nothing unmounts above opacity 0.
    half = hold({**still, "root/chip": (10, 10, 50, 20, 1)}, 5) + [{**still, "root/chip": (10, 10, 50, 20, op)} for op in ease(1, 0.4, 6)] + hold(still, 10)
    assert "cut-exit" in kinds(half), "a fade cut short at 0.4"
    full = hold({**still, "root/chip": (10, 10, 50, 20, 1)}, 5) + [{**still, "root/chip": (10, 10, 50, 20, op)} for op in ease(1, 0, 12)] + hold(still, 10)
    assert not {"cut-exit", "pop-out"} & set(kinds(full)), "a fade to 0, then gone"

    # an instant scroll (wrapping to a long list's end) mounts the rows it lands on and drops the
    # rows it left: the scroll, not pops. The same swap with no scroll pops.
    top = {**still, "root/list/a": (0, 0, 200, 30, 1), "root/list/b": (0, 30, 200, 30, 1)}
    end = {**still, "root/list/y": (0, 0, 200, 30, 1), "root/list/z": (0, 30, 200, 30, 1)}
    swap = hold(top, 10) + hold(end, 20)
    assert {"pop-in", "pop-out"} <= set(kinds(swap)), "rows swapped in place"
    assert not {"pop-in", "pop-out"} & set(kinds(swap, flags={"root/list/y": 4 | 8, "root/list/z": 4 | 8})), "rows scrolled in"

    # stable identity: closing row a must remove a, not b (index keys remove the last row).
    rows = lambda *ks: {**still, **{f"root/list/{k}": (0, 30 * i, 200, 30, 1) for i, k in enumerate(ks)}, "root/list": (0, 0, 200, 90, 1)}
    close = [{"t": 0.1, "type": "tap", "key": "a/close", "expect": "leaves", "element": "root/list/a"}]
    assert "wrong-exit" in kinds(hold(rows("a", "b", "c"), 12) + hold(rows("a", "b"), 40), close), "the last row left instead"
    assert "wrong-exit" not in kinds(hold(rows("a", "b", "c"), 12) + hold(rows("b", "c"), 40), close), "row a left"

    # smooth: a release that lands in one frame snaps.
    release = [{"t": 0.1, "type": "drag", "key": "card", "dx": 50, "dur": 0.1, "expect": "smooth", "element": "root/card"}]
    assert "snap" in kinds(hold(at(100, 100), 30) + hold(at(200, 100), 40), release), "a landing in one frame"
    assert "snap" not in kinds(hold(at(100, 100), 30) + [at(x, 100) for x in ease(100, 200, 24)] + hold(at(200, 100), 20), release), "a glide"

    assert not jumps(sheen((1.0, 0.0), (1.0, 5.0))), "a wrap from clipped away isn't a jump"
    assert not jumps(sheen((0.0, 10.0), (1.0, 5.0))), "a wrap from transparent isn't a jump"
    assert not jumps(sheen((1.0, 10.0), (0.0, 10.0))), "a wrap into transparent isn't a jump"
    found = jumps(sheen((1.0, 10.0), (1.0, 10.0)))
    assert len(found) == 1 and found[0]["at"] == "10", f"a wrap in sight is: {found}"
    print("ok")


if __name__ == "__main__":
    main()
