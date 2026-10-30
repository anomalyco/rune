#!/usr/bin/env python3
"""Motion audit of Rune traces (`--trace out.jsonl --trace-all`).

Reads each element's presented rect, opacity and clipped rect per frame, the tracks the
motion store sampled (`tk`) and why the frame was animating (`quality.why`), and lists what
looks wrong:

  invisible-work   tracks sampled while their element can't be seen (opacity 0, clipped
                   away or off the window) for several frames in a row
  idle-frames      frames that asked for another frame but changed nothing traced
  never-settles    still animating at the end of the run, long after the last input
  pop-in/pop-out   mounted or unmounted at full opacity with no transition
  cut-exit         unmounted partway through its fade (nothing leaves above opacity 0)
  jump             a rect edge moves far in one frame between still frames (no interpolation)
  step             one frame of a smooth motion moves much more than its neighbours
  sibling-jump     a jump in a sibling of something that just mounted or unmounted
  overshoot        an edge passes its rest value and comes back
  jiggle           an edge reverses direction twice or more within one motion
  reversal         one motion heads the wrong way first, or goes out and comes back
  stall            one motion's speed drops for a frame and picks up again (a retarget that
                   lost part of its velocity)
  dog-leg          a move's route is much longer than the straight line between its ends, on
                   screen and against its parent
  latency          input whose first visible response comes frames later
  hitch            frame CPU over budget during motion

With `--script S.json` before a trace, the script's events can say what they expect
(`"expect": "instant" | "follow" | "smooth" | "leaves"`, see `expectations`), which adds
not-instant, lags-hand, snap and wrong-exit findings and makes cuts inside an instant
window intended.

Usage: scripts/motion-audit/analyze.py [--script S.json] TRACE.jsonl... [--json out.json]
       [--allow allow.json] [--fail] [--quiet]
`--allow` takes rules `{"trace", "kind" (or a list), "element" (a substring), "reason"}`;
every rule needs a reason. Exit code is 0 unless `--fail` and a finding isn't allowed.
"""

import bisect
import json
import math
import statistics
import sys
from collections import defaultdict

VISIBLE_OPACITY = 0.01
POP_OPACITY = 0.5
JUMP_PT = 12.0
# Frames at rest a motion segment bridges (see the segments in `analyze`).
SEG_GAP = 2
# A one-frame speed dip worth reporting: at least this many points a frame, and this share of
# the slower neighbour's speed (a spring's speed never has a corner).
STALL_PT = 0.5
STALL_SHARE = 0.25
MIN_AREA = 1.0
SCROLLING = 4  # flags: under a scroll viewport whose offset moved this frame
BLANK = 64  # flags: draws nothing and holds nothing (a hit or hover zone, a spacer)
SETTLE_TAIL_MS = 250.0  # a motion's own sub-visible tail after it hides what it moves


def short(name, parts=3):
    segs = name.split("/")
    return "/".join(segs[-parts:]) if len(segs) > parts else name


class El:
    __slots__ = ("id", "name", "parent", "rect", "op", "flags", "vis", "born", "hist", "ever_drawn")

    def __init__(self, id, name, parent):
        self.id, self.name, self.parent = id, name, parent
        self.rect = None
        self.op = 1.0
        self.flags = 0
        self.vis = None
        self.born = None
        self.ever_drawn = False  # had area at some point (spacers never do)
        self.hist = []  # (frame, x, y, w, h, op, flags, visible-area-fraction, on-screen)


def area(r):
    return max(r[2], 0) * max(r[3], 0)


def on_screen(r, W, H):
    return r[0] < W and r[1] < H and r[0] + r[2] > 0 and r[1] + r[3] > 0 and area(r) > 0


def visible_part(e, W, H):
    r = e.vis if e.vis is not None else e.rect
    x0, y0 = max(r[0], 0), max(r[1], 0)
    x1, y1 = min(r[0] + r[2], W), min(r[1] + r[3], H)
    return [x0, y0, max(x1 - x0, 0), max(y1 - y0, 0)]


def seen(e, W, H):
    return e.op > VISIBLE_OPACITY and area(visible_part(e, W, H)) >= MIN_AREA


def load(path):
    frames = []
    with open(path) as f:
        header = json.loads(f.readline())
        for line in f:
            line = line.strip()
            if line:
                frames.append(json.loads(line))
    return header, frames


def runs(frames_list):
    """Consecutive frame indices → [(start, end)]."""
    out = []
    for k in sorted(frames_list):
        if out and k == out[-1][1] + 1:
            out[-1][1] = k
        else:
            out.append([k, k])
    return [tuple(r) for r in out]


def fmt_runs(rs, limit=4):
    s = ", ".join(f"{a}" if a == b else f"{a}–{b}" for a, b in rs[:limit])
    return s + (f" (+{len(rs) - limit} more)" if len(rs) > limit else "")


def loop_ids(els, moved):
    """A loop (a spinner, a sheen, a busy ring's dots) keeps changing on most frames it's alive,
    as one element or as a group of siblings taking turns. Its wraps and turns are its design."""
    busy = defaultdict(set)
    group_busy = defaultdict(set)
    for j, moves in enumerate(moved):
        for id in moves:
            busy[id].add(j)
            if id in els:
                group_busy[els[id].parent].add(j)
    life = {e.id: len(e.hist) for e in els.values()}
    out = {id for id in busy if life.get(id, 0) >= 60 and len(busy[id]) >= 0.5 * life[id]}
    groups = {p for p, ks in group_busy.items() if p in life and life[p] >= 60 and len(ks) >= 0.5 * life[p]}
    out |= {e.id for e in els.values() if e.parent in groups}
    # What a loop carries moves with it.
    grew = True
    while grew:
        more = {e.id for e in els.values() if e.parent in out and e.id not in out}
        grew = bool(more)
        out |= more
    return out


def stalled(e, parent, edge, pts, travel):
    """The first frame of one motion (`pts`, consecutive history rows) whose speed along
    `travel` dips below both neighbours' and recovers, with the three speeds; one that rides
    its parent is the parent's to report."""
    ph = {x[0]: x for x in parent.hist} if parent else {}
    sign = math.copysign(1, travel)
    for m in range(2, len(pts) - 1):
        if pts[m + 1][0] - pts[m - 2][0] != 3:
            continue
        u = [(pts[m + j][edge] - pts[m + j - 1][edge]) * sign for j in (-1, 0, 1)]
        floor = min(u[0], u[2])
        if floor <= STALL_PT or u[1] >= floor - max(STALL_PT, STALL_SHARE * floor):
            continue
        k = pts[m][0]
        if k in ph and k - 1 in ph and abs((ph[k][edge] - ph[k - 1][edge]) - (pts[m][edge] - pts[m - 1][edge])) < 0.05:
            continue
        return k, u
    return None


def analyze(path, script=None):
    header, frames = load(path)
    W, H, hz = header["width"], header["height"], header["hz"]
    dt_ms = 1000.0 / hz
    els = {}
    alive = set()
    findings = []
    touched_invisible = defaultdict(list)  # (name, prop) -> frames
    sampled = defaultdict(set)  # (id, prop) -> frames its track ran
    touched_untraced = defaultdict(int)
    idle = []
    animating = []
    inputs = []  # (frame, event)
    births = defaultdict(list)  # frame -> ids
    deaths = defaultdict(list)  # frame -> (id, last state)
    scrolled = defaultdict(set)  # frame -> containers (by path) of elements a scroll moved
    cpu = []
    changed_by_frame = []
    t_ms = []
    changes = []  # frame -> {id: props whose value changed or whose track ran}
    prev = {}  # id -> (x, y, w, h, opacity)

    for k, fr in enumerate(frames):
        t_ms.append(fr.get("t", k * dt_ms))
        moves = defaultdict(set)
        changes.append(moves)
        for m in fr.get("m", []):
            e = els.get(m["id"])
            if e is None:
                e = els[m["id"]] = El(m["id"], m["nm"], m.get("pv"))
            else:
                e.name, e.parent = m["nm"], m.get("pv", e.parent)
        changed = set()
        for row in fr.get("e", []):
            id = row[0]
            e = els[id]
            e.rect = row[1:5]
            e.op = row[5]
            e.flags = row[7]
            e.vis = row[8:12] if len(row) >= 12 else None
            e.ever_drawn |= area(e.rect) >= MIN_AREA and not e.flags & BLANK
            old = prev.get(id)
            if old is not None and id in alive:
                if abs(row[1] - old[0]) > 0.01 or abs(row[2] - old[1]) > 0.01:
                    moves[id].add("position")
                if abs(row[3] - old[2]) > 0.01 or abs(row[4] - old[3]) > 0.01:
                    moves[id].add("size")
                if abs(row[5] - old[4]) > 0.001:
                    moves[id].add("opacity")
            prev[id] = tuple(row[1:6])
            if id not in alive:
                alive.add(id)
                e.born = k
                if k > 0:
                    births[k].append(id)
            changed.add(id)
        for id in fr.get("g", []):
            if id in alive:
                alive.discard(id)
                e = els[id]
                deaths[k].append((id, e.rect, e.op, seen(e, W, H) and not e.flags & BLANK))
        for id in alive:
            e = els[id]
            vp = visible_part(e, W, H)
            frac = area(vp) / area(e.rect) if area(e.rect) > 0 else 0.0
            e.hist.append((k, *e.rect, e.op, e.flags, frac, on_screen(e.rect, W, H)))
            if e.flags & SCROLLING:
                scrolled[k].add(e.name.rpartition("/")[0])
        q = fr.get("quality", {})
        cpu.append(q.get("cpu_ms", 0.0))
        anim = bool(q.get("animating"))
        animating.append(anim)
        visible_change = any(seen(els[i], W, H) or i in births.get(k, []) for i in changed)
        changed_by_frame.append(visible_change)
        for ev in fr.get("ev", []):
            if not ev.startswith(("settled", "completed")):
                inputs.append((k, ev))
        tracks = fr.get("tk", [])
        why = tuple(q.get("why", []))
        # A loop's curve flattens at its turns, where a frame moves it less than the trace shows.
        if anim and not changed and k > 0 and not any(a for _, _, a in tracks) and why != ("loop",):
            idle.append((k, why))
        for who, prop, active in tracks:
            if not active:
                continue  # set without animating: no motion this frame
            if isinstance(who, str):
                # An untraced container (a layout-only stack) is seen if what it holds is.
                if not any(els[i].name.startswith(who + "/") and seen(els[i], W, H) for i in alive):
                    touched_untraced[(short(who), prop)] += 1
                continue
            sampled[(who, prop)].add(k)
            moves[who].add(prop)
            e = els.get(who)
            if e is None or who not in alive:
                touched_untraced[(f"#{who}", prop)] += 1
                continue
            if not seen(e, W, H) and e.ever_drawn:
                touched_invisible[(e.name, prop, who)].append(k)

    n = len(frames)
    acts = script_inputs(script) if script else []
    loops = loop_ids(els, changes)

    def frame_of(t):
        return min(bisect.bisect_left(t_ms, t * 1000.0 - 0.01), n - 1)

    input_frames = sorted({frame_of(t) for a in acts for t in a["times"]} | {k for k, _ in inputs})

    def interrupted(k0, k1):
        """An input landed mid-motion (a retarget may turn it around)."""
        i = bisect.bisect_right(input_frames, k0)
        return i < len(input_frames) and input_frames[i] <= k1

    # invisible work: report the outermost element of a subtree that went unseen together
    def covered(id, prop, ks):
        p = els[id].parent
        while p is not None:
            for prop2 in {prop, "frame", "presence", "opacity"}:
                if (p, prop2) in keys and set(ks) <= set(keys[(p, prop2)]):
                    return True
            p = els[p].parent if p in els else None
        return False
    def tail(id, prop, a, b):
        # The motion that just hid the element (a fade to 0, a collapse, a slide out), running
        # out its last sub-visible stretch to its settle threshold, then stopping.
        before = [st for st in els[id].hist if st[0] == a - 1]
        was_seen = bool(before) and before[0][5] > VISIBLE_OPACITY and before[0][7] * before[0][3] * before[0][4] >= MIN_AREA
        return was_seen and b + 1 not in sampled[(id, prop)] and (b - a + 1) * dt_ms <= SETTLE_TAIL_MS
    for key, ks in list(touched_invisible.items()):
        (name, prop, id) = key
        rest = [k for a, b in runs(ks) if not tail(id, prop, a, b) for k in range(a, b + 1)]
        if rest:
            touched_invisible[key] = rest
        else:
            del touched_invisible[key]
    keys = {(id, prop): ks for (name, prop, id), ks in touched_invisible.items()}
    for (name, prop, id), ks in touched_invisible.items():
        if covered(id, prop, ks):
            continue
        rs = runs(ks)
        longest = max(b - a + 1 for a, b in rs)
        if longest < 6:
            continue
        e = els[id]
        kset = set(ks)
        unseen = [st for st in e.hist if st[0] in kset]
        why = []
        if any(st[5] <= VISIBLE_OPACITY for st in unseen):
            op = min(st[5] for st in unseen)
            why.append(f"opacity {op}")
        if any(st[7] == 0 and st[8] for st in unseen):
            why.append("clipped away")
        if any(not st[8] for st in unseen):
            why.append("off-window")
        findings.append(dict(kind="invisible-work", element=name, prop=prop, frames=len(ks), longest=longest,
                             at=fmt_runs(rs), detail=f"{prop} sampled on {len(ks)} frames while unseen ({', '.join(why) or 'unseen'})"))
    for (who, prop), count in touched_untraced.items():
        if count >= 6:
            findings.append(dict(kind="invisible-work", element=who, prop=prop, frames=count, longest=count, at="",
                                 detail=f"{prop} sampled on {count} frames for a node not in the element tree"))

    # idle frames and settling
    if idle:
        by_why = defaultdict(list)
        for k, why in idle:
            by_why[why].append(k)
        for why, ks in by_why.items():
            rs = runs(ks)
            findings.append(dict(kind="idle-frames", element="(frame)", frames=len(ks), longest=max(b - a + 1 for a, b in rs),
                                 at=fmt_runs(rs), detail=f"{len(ks)} frames animating ({'+'.join(why) or '?'}) with no traced change"))
    last_input = max((k for k, _ in inputs), default=0)
    tail = [k for k in range(n) if animating[k]]
    # A loop (a spinner) runs for as long as it's shown: only other motion fails to settle.
    quiet = int(500 / dt_ms)
    only_loops = frames[-1].get("quality", {}).get("why", []) == ["loop"] and not any(
        a and who not in loops for fr in frames[-quiet:] for who, _, a in fr.get("tk", []))
    if animating and animating[-1] and (n - 1 - last_input) * dt_ms > 1000 and not only_loops:
        q = frames[-1].get("quality", {})
        start = n - 1
        while start > 0 and animating[start - 1]:
            start -= 1
        busy = defaultdict(int)
        for fr in frames[start:]:
            for who, prop, _ in fr.get("tk", []):
                busy[(els[who].name if isinstance(who, int) and who in els else str(who), prop)] += 1
        top = sorted(busy.items(), key=lambda kv: -kv[1])[:4]
        findings.append(dict(kind="never-settles", element="(frame)", frames=n - start, longest=n - start, at=f"{start}–{n - 1}",
                             detail=f"still animating ({'+'.join(q.get('why', []))}) {((n - 1 - last_input) * dt_ms / 1000):.1f}s after the last input; "
                                    + ("busiest tracks: " + "; ".join(f"{short(nm)} {p} ×{c}" for (nm, p), c in top) if top else "no store tracks")))

    # pops
    def top_level(ids, k=None, table=None):
        # By path too: a layout-only parent (an untraced stack) breaks the id chain.
        s = set(ids)
        names = {els[i].name for i in ids}
        def under(i):
            nm = els[i].name
            return any(nm[:j] in names for j in range(len(nm)) if nm[j] == "/")
        return [i for i in ids if els[i].parent not in s and not under(i)]

    for k, ids in births.items():
        for id in top_level(ids, k, births):
            e = els[id]
            if id in loops:
                continue
            h = [x for x in e.hist if x[0] >= k][:3]
            if not h:
                continue
            k0, x, y, w, hh, op, flags, frac, ons = h[0]
            # A viewport's content entering as it scrolls (a virtual list mounting the rows an
            # instant scroll lands on) is the scroll, which jumps already exempt, not a pop.
            if op < POP_OPACITY or frac == 0 or not ons or w * hh < 4 or flags & (SCROLLING | BLANK):
                continue
            grows = len(h) > 1 and (abs(h[1][3] - w) > 0.5 or abs(h[1][4] - hh) > 0.5 or abs(h[1][5] - op) > 0.01)
            intent = flags >> 8
            if grows or (intent & (16 | 4 | 32)):
                continue
            moved = len(h) > 1 and (abs(h[1][1] - x) > JUMP_PT or abs(h[1][2] - y) > JUMP_PT)
            findings.append(dict(kind="pop-in" if not moved else "appear-then-jump", element=e.name, frames=1, longest=1, at=str(k),
                                 detail=f"mounted at opacity {op} size {w:.0f}×{hh:.0f} at ({x:.0f},{y:.0f})"
                                        + (f", then jumped to ({h[1][1]:.0f},{h[1][2]:.0f})" if moved else " with no transition")))
    for k, items in deaths.items():
        tops = set(top_level([i for i, *_ in items]))
        for id, rect, op, was_seen in items:
            # Leaving as its viewport scrolls (rows a jump left behind) is the scroll too.
            if id not in tops or not was_seen or els[id].name.rpartition("/")[0] in scrolled[k]:
                continue
            # Nothing unmounts above opacity 0: a fade cut short is a smaller pop.
            findings.append(dict(kind="pop-out" if op >= POP_OPACITY else "cut-exit", element=els[id].name, frames=1, longest=1, at=str(k),
                                 detail=f"unmounted at opacity {op} size {rect[2]:.0f}×{rect[3]:.0f} at ({rect[0]:.0f},{rect[1]:.0f})"
                                        + ("" if op >= POP_OPACITY else ", before its fade reached 0")))

    # geometry: jumps, steps, overshoot, jiggle
    changed_parents = defaultdict(set)  # frame -> parents with births/deaths
    for k, ids in births.items():
        for i in ids:
            changed_parents[k].add(els[i].parent)
    for k, items in deaths.items():
        for i, *_ in items:
            changed_parents[k].add(els[i].parent)
    pressing = [bool(fr.get("k")) for fr in frames]
    for e in els.values():
        h = e.hist
        if len(h) < 3 or e.id in loops:
            continue
        # Only report the outermost element of a group moving together.
        for edge, label in ((1, "x"), (2, "y"), (3, "w"), (4, "h")):
            d = [h[i][edge] - h[i - 1][edge] if h[i][0] == h[i - 1][0] + 1 else 0.0 for i in range(1, len(h))]
            for i in range(1, len(h)):
                k = h[i][0]
                step = d[i - 1]
                if abs(step) < JUMP_PT:
                    continue
                st, prev = h[i], h[i - 1]
                if pressing[k] or st[6] & (4 | BLANK):
                    continue
                # A move nobody sees on either side isn't a jump: a loop wrapping while it's
                # transparent or clipped away (a shimmer's sheen going back to its start).
                if any(s[5] <= 0.05 or s[7] == 0 or not s[8] for s in (prev, st)):
                    continue
                intent = st[6] >> 8
                if intent & (1 if label in "xy" else 2):
                    continue
                before = d[i - 2] if i >= 2 else 0.0
                after = d[i] if i < len(d) else 0.0
                if abs(before) < 1.0 and abs(after) < 1.0:
                    kind = "sibling-jump" if any(els.get(e.parent) and p == e.parent for p in changed_parents.get(k, ())) else "jump"
                elif abs(step) > 3 * max(abs(before), abs(after), 1.0):
                    kind = "step"
                else:
                    continue
                parent = els.get(e.parent)
                if parent and any(x[0] == k for x in parent.hist):
                    ph = {x[0]: x for x in parent.hist}
                    if k in ph and k - 1 in ph and abs((ph[k][edge] - ph[k - 1][edge]) - step) < 0.5 and label in "xy":
                        continue  # rides its parent; the parent reports it
                findings.append(dict(kind=kind, element=e.name, frames=1, longest=1, at=str(k),
                                     detail=f"{label} {h[i - 1][edge]:.1f} → {st[edge]:.1f} ({step:+.1f}pt) in one frame; neighbours {before:+.1f}/{after:+.1f}"))
            # motion segments. A motion may rest a frame or two where it turns (an out-and-back
            # through its start), so up to SEG_GAP frames at rest don't end one.
            seg = []
            for i in range(1, len(h)):
                moving = h[i][0] - h[i - 1][0] <= SEG_GAP + 1 and abs(h[i][edge] - h[i - 1][edge]) > 0.01
                if moving:
                    seg.append(i)
                elif seg and h[i][0] - h[seg[-1]][0] <= SEG_GAP and i < len(h) - 1:
                    continue
                if (not moving or i == len(h) - 1) and len(seg) >= 4:
                    vals = [h[seg[0] - 1][edge]] + [h[j][edge] for j in seg]
                    if all(h[j][5] <= 0.05 or not h[j][8] for j in seg):
                        seg = []
                        continue
                    if any(pressing[h[j][0]] or h[j][6] & (4 | BLANK) for j in seg):
                        seg = []
                        continue
                    rest = vals[-1]
                    start = vals[0]
                    travel = rest - start
                    signs = [math.copysign(1, b - a) for a, b in zip(vals, vals[1:]) if abs(b - a) > 0.05]
                    reversals = sum(1 for a, b in zip(signs, signs[1:]) if a != b)
                    past = max(((v - rest) * math.copysign(1, travel) for v in vals), default=0) if abs(travel) > 0.5 else 0
                    k0, k1 = h[seg[0]][0], h[seg[-1]][0]
                    found = len(findings)
                    if past > 1.0:
                        findings.append(dict(kind="overshoot", element=e.name, frames=k1 - k0 + 1, longest=k1 - k0 + 1, at=f"{k0}–{k1}",
                                             detail=f"{label} {start:.1f} → {rest:.1f}, overshoots by {past:.1f}pt ({100 * past / abs(travel):.0f}%)"))
                    elif reversals >= 2:
                        amp = max(vals) - min(vals)
                        if amp > 0.5:
                            findings.append(dict(kind="jiggle", element=e.name, frames=k1 - k0 + 1, longest=k1 - k0 + 1, at=f"{k0}–{k1}",
                                                 detail=f"{label} reverses {reversals}× over {amp:.1f}pt ({start:.1f} → {rest:.1f})"))
                    elif reversals == 1 and not interrupted(k0, k1):
                        # One monotonic path: no wind-up the wrong way, no out-and-back.
                        if abs(travel) > 0.5:
                            back = max((start - v) * math.copysign(1, travel) for v in vals)
                            what = f"{label} {start:.1f} → {rest:.1f} heads {back:.1f}pt the wrong way first"
                        else:
                            back = max(abs(v - start) for v in vals)
                            what = f"{label} goes {back:.1f}pt out from {start:.1f} and back"
                        if back > 1.0:
                            findings.append(dict(kind="reversal", element=e.name, frames=k1 - k0 + 1, longest=k1 - k0 + 1, at=f"{k0}–{k1}",
                                                 detail=what))
                    if len(findings) == found and abs(travel) > 0.5:
                        stall = stalled(e, els.get(e.parent), edge, [h[seg[0] - 1]] + [h[j] for j in seg], travel)
                        if stall:
                            k, u = stall
                            findings.append(dict(kind="stall", element=e.name, frames=1, longest=1, at=str(k),
                                                 detail=f"{label} speed {u[0]:.1f} → {u[1]:.1f} → {u[2]:.1f} pt/frame ({start:.1f} → {rest:.1f})"))
                if not moving:
                    seg = []
        # One straight path: the top-left corner's route against its chord, per continuous move.
        seg = []
        ph = {x[0]: x for x in parent.hist} if (parent := els.get(e.parent)) else {}
        for i in range(1, len(h) + 1):
            if i < len(h) and h[i][0] == h[i - 1][0] + 1 and (abs(h[i][1] - h[i - 1][1]) > 0.01 or abs(h[i][2] - h[i - 1][2]) > 0.01):
                seg.append(i)
                continue
            if len(seg) >= 4:
                pts = [h[seg[0] - 1]] + [h[j] for j in seg]
                k0, k1 = pts[1][0], pts[-1][0]
                hidden = all(p[5] <= 0.05 or not p[8] for p in pts)
                held = any(pressing[p[0]] or p[6] & (4 | BLANK) for p in pts)
                rides = all(p[0] in ph and p[0] - 1 in ph and abs((ph[p[0]][1] - ph[p[0] - 1][1]) - (p[1] - q[1])) < 0.05
                            and abs((ph[p[0]][2] - ph[p[0] - 1][2]) - (p[2] - q[2])) < 0.05 for q, p in zip(pts, pts[1:]))
                if not (hidden or held or rides or interrupted(k0, k1)):
                    route = sum(math.hypot(p[1] - q[1], p[2] - q[2]) for q, p in zip(pts, pts[1:]))
                    chord = math.hypot(pts[-1][1] - pts[0][1], pts[-1][2] - pts[0][2])
                    # Its own move on a moving parent (a chevron sliding along a row as the list
                    # scrolls) bends only by the parent's: straight against the parent is one path.
                    if all(p[0] in ph for p in pts):
                        rel = [(p[1] - ph[p[0]][1], p[2] - ph[p[0]][2]) for p in pts]
                        own = sum(math.hypot(b[0] - a[0], b[1] - a[1]) for a, b in zip(rel, rel[1:]))
                        bent = own > 1.2 * math.hypot(rel[-1][0] - rel[0][0], rel[-1][1] - rel[0][1])
                        if own < route - 0.01 and (own <= JUMP_PT or not bent):
                            route = chord
                    if chord > JUMP_PT and route > 1.2 * chord:
                        findings.append(dict(kind="dog-leg", element=e.name, frames=k1 - k0 + 1, longest=k1 - k0 + 1, at=f"{k0}–{k1}",
                                             detail=f"({pts[0][1]:.0f},{pts[0][2]:.0f}) → ({pts[-1][1]:.0f},{pts[-1][2]:.0f}) travels {route:.0f}pt for a {chord:.0f}pt move"))
            seg = []

    # latency: input to first visible change
    for k, ev in inputs:
        if not ev.startswith(("press", "key", "submit", "double press", "secondary press")):
            continue
        j = next((j for j in range(k, min(n, k + 30)) if changed_by_frame[j]), None)
        if j is None:
            findings.append(dict(kind="latency", element="(input)", frames=30, longest=30, at=str(k), detail=f"{ev}: no visible change within 30 frames"))
        elif j - k >= 2:
            findings.append(dict(kind="latency", element="(input)", frames=j - k, longest=j - k, at=f"{k}→{j}",
                                 detail=f"{ev}: first visible change {j - k} frames ({(j - k) * dt_ms:.0f} ms) later"))

    # hitches during motion
    moving_frames = [k for k in range(1, n) if animating[k - 1]]
    if moving_frames:
        vals = [cpu[k] for k in moving_frames]
        med = statistics.median(vals)
        budget = dt_ms
        bad = [k for k in moving_frames if cpu[k] > max(budget, 3 * med)]
        if bad:
            worst = max(bad, key=lambda k: cpu[k])
            findings.append(dict(kind="hitch", element="(frame)", frames=len(bad), longest=len(bad), at=fmt_runs(runs(bad)),
                                 detail=f"{len(bad)} animating frames over {budget:.1f} ms CPU (median {med:.2f}, worst {cpu[worst]:.1f} at frame {worst})"))

    if acts:
        findings = expectations(acts, findings, els, changes, loops, deaths, n, hz, frame_of)

    return dict(trace=path, frames=n, hz=hz, inputs=[(k, ev) for k, ev in inputs], findings=findings,
                animating_frames=sum(animating), idle_frames=len(idle))


def script_inputs(script):
    """The script's events with the times their inputs land (a typed string is one per key,
    a tap a press and a release, a drag its press and its release)."""
    events = script.get("events", []) if isinstance(script, dict) else script
    out = []
    for i, e in enumerate(events):
        t = float(e.get("t", 0.0))
        ty = e.get("type", "tap")
        end = t
        if ty == "type" and "paste" not in e:
            every = e.get("every", 0.035)
            times = [t + every * j for j in range(max(len(e.get("text", "")), 1))]
        elif ty in ("tap", "click", "secondary-click"):
            times = [t, t + e.get("hold", 0.08)]
        elif ty in ("double-click", "double-tap", "triple-click"):
            gap = max(e.get("gap", 0.15), e.get("hold", 0.06) + 0.01)
            times = [t + gap * j for j in range(3 if ty == "triple-click" else 2)]
        elif ty == "drag":
            end = t + max(e.get("dur", 0.3), 0.01) + 0.02
            times = [t, end]
        elif ty == "scroll":
            end = t + max(e.get("dur", 0.2), 0.01)
            times = [t, end]
        else:
            times = [t]
        expect = e.get("expect", [])
        out.append(dict(index=i, event=e, type=ty, times=times, end=max(times[-1], end),
                        expect=[expect] if isinstance(expect, str) else list(expect)))
    return out


def describe(e):
    ty = e.get("type", "tap")
    mods = "".join(s for m, s in (("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")) if e.get(m))
    if ty in ("key", "key-up"):
        return f"key {mods}{e.get('name', '')}"
    if ty == "type":
        return f"type {e.get('text', '')!r}"
    where = e.get("key") or (f"({e['x']:.0f},{e['y']:.0f})" if "x" in e else "")
    return f"{ty} {where}".strip()


def named(name, key):
    return name == key or name.endswith("/" + key)


def expectations(acts, findings, els, moved, background, deaths, n, hz, frame_of):
    """Checks the script's `expect`ations and drops generic findings they make intended.

      instant  everything the input changes lands in the frame (a cut): nothing in `scope`
               (an element name part, default all) still changes two frames after it
      follow   a drag's `element` moves with the pointer, 1:1, on the drag's axes (or `prop`)
      smooth   after the input (a drag's release), `element` travels over several frames;
               no single frame covers most of the move, and it doesn't land at once
      leaves   `element` is the one that leaves; no sibling leaves in its place
    """
    times = sorted({t for a in acts for t in a["times"]})
    out = []
    instant_windows = []  # (k0, k1, scope)
    dying = defaultdict(list)
    for k, items in deaths.items():
        for id, rect, op, was_seen in items:
            dying[id].append((k, op))

    def next_time(t):
        i = bisect.bisect_right(times, t + 1e-6)
        return times[i] if i < len(times) else None

    def window(t, cap):
        k0 = frame_of(t)
        nt = next_time(t)
        k1 = min(frame_of(nt) if nt is not None else n, k0 + int(cap * hz), n)
        return k0, max(k1, k0 + 1)

    def find(key, k0, k1):
        cands = [e for e in els.values() if named(e.name, key) and any(k0 - 1 <= st[0] < k1 for st in e.hist)]
        return min(cands, key=lambda e: (e.flags & BLANK != 0, len(e.name))) if cands else None

    def ancestors(id):
        seen_ = set()
        p = els[id].parent if id in els else None
        while p is not None and p not in seen_:
            seen_.add(p)
            yield p
            p = els[p].parent if p in els else None

    def missing(a, key):
        out.append(dict(kind="expect-missing", element=key, frames=0, longest=0, at=str(frame_of(a["times"][0])),
                        detail=f"{describe(a['event'])}: no element {key!r} to check '{'/'.join(a['expect'])}' on"))

    for a in acts:
        e, label = a["event"], describe(a["event"])
        if "instant" in a["expect"]:
            scope = e.get("scope", "")
            bad = {}  # id -> (frames, props)
            for t in a["times"]:
                k0, k1 = window(t, 0.6)
                instant_windows.append((k0, k1, scope))
                runs_ = defaultdict(list)
                # A motion already running when the input lands (an entrance settling) isn't its answer.
                already = defaultdict(set)
                for j in range(max(k0 - 2, 0), k0):
                    for id, props in moved[j].items():
                        already[id] |= props
                for j in range(k0, k1):
                    for id, props in moved[j].items():
                        el = els.get(id)
                        # A pressed control's feedback answers the finger, not the input's effect.
                        props = props - {"press"} - already[id]
                        if el is None or not props or id in background or el.flags & BLANK or scope not in el.name:
                            continue
                        runs_[id].append((j, props))
                for id, hits in runs_.items():
                    ks = [j for j, _ in hits]
                    # Two changes in a row, the later two frames or more after the input: motion.
                    late = [j for j in ks if j - 1 in ks and j >= k0 + 2]
                    if late:
                        frames_, props = bad.get(id, ([], set()))
                        props = props | {p for j, ps in hits if j in late or j + 1 in late for p in ps}
                        bad[id] = (frames_ + [j for j in ks if j >= k0 + 1], props)
            tops = [id for id in bad if not any(p in bad for p in ancestors(id))]
            for id in sorted(tops, key=lambda i: -len(bad[i][0]))[:4]:
                ks, props = bad[id]
                rs = runs(ks)
                out.append(dict(kind="not-instant", element=els[id].name, frames=len(ks), longest=max(b - a_ + 1 for a_, b in rs), at=fmt_runs(rs, 2),
                                detail=f"{label}: {', '.join(sorted(props))} still changing {max(b - a_ + 1 for a_, b in rs)} frames in a row (expected a cut)"))
            if len(tops) > 4:
                out.append(dict(kind="not-instant", element=f"(+{len(tops) - 4} more)", frames=0, longest=0, at="", detail=f"{label}: more elements animate"))
        if "follow" in a["expect"]:
            key = e.get("element") or e.get("key", "")
            t0, dur = float(e.get("t", 0.0)), max(e.get("dur", 0.3), 0.01)
            k0, k1 = frame_of(t0), frame_of(t0 + dur + 0.02)
            el = find(key, k0, k1)
            if el is None:
                missing(a, key)
            else:
                steps = math.ceil(dur * 120)

                def hand(t):
                    s = max(0, min(steps, math.floor((t - t0) / dur * steps + 1e-6)))
                    f = s / steps
                    f = f * f * (3 - 2 * f)
                    return e.get("dx", 0.0) * f, e.get("dy", 0.0) * f
                axes = [e["prop"]] if "prop" in e else [c for c, d in (("x", "dx"), ("y", "dy")) if e.get(d)]
                idx = {"x": 1, "y": 2, "w": 3, "h": 4}
                pos = {st[0]: st for st in el.hist}
                off, started = [], False
                drift = defaultdict(float)
                for j in range(k0 + 1, k1):
                    if j not in pos or j - 1 not in pos:
                        continue
                    tj, tp = j / hz, (j - 1) / hz
                    for c in axes:
                        de = pos[j][idx[c]] - pos[j - 1][idx[c]]
                        dh = hand(tj)[0 if c in "xw" else 1] - hand(tp)[0 if c in "xw" else 1]
                        # The first move picks up what the hand moved inside the drag's slop.
                        pickup = not started and abs(de) > 0.05
                        started |= pickup
                        # Held at a limit (the list's end, a minimum width), it may stop; while
                        # it moves, it moves with the hand, so a spring trailing it drifts off.
                        if started and not pickup and abs(de) > 0.05:
                            drift[c] += de - dh
                            if abs(drift[c]) > 2.0 or abs(de - dh) > 1.0:
                                off.append((j, drift[c] if abs(drift[c]) > 2.0 else de - dh))
                if not started and any(abs(v) > 1.0 for v in hand(t0 + dur)):
                    out.append(dict(kind="lags-hand", element=el.name, frames=0, longest=0, at=str(k0),
                                    detail=f"{label}: doesn't move while the pointer drags it"))
                elif len(off) >= 2:
                    worst = max(off, key=lambda o: abs(o[1]))
                    out.append(dict(kind="lags-hand", element=el.name, frames=len(off), longest=len(off), at=fmt_runs(runs([j for j, _ in off]), 2),
                                    detail=f"{label}: moves off the pointer on {len(off)} frames (worst {worst[1]:+.1f}pt at frame {worst[0]}); a drag follows the hand 1:1"))
        if "smooth" in a["expect"]:
            key = e.get("element") or e.get("key", "")
            k0, k1 = window(a["end"], 1.2)
            el = find(key, k0, k1)
            if el is None:
                missing(a, key)
            else:
                h = [st for st in el.hist if k0 - 1 <= st[0] < k1]
                born = el.born is not None and k0 - 1 <= el.born < k1
                worst = None
                travel = 0.0
                for c, label_c in ((1, "x"), (2, "y"), (3, "w"), (4, "h")):
                    vals = [st[c] for st in h]
                    total = vals[-1] - vals[0] if vals else 0.0
                    travel = max(travel, abs(total))
                    steps_ = [b - a_ for a_, b in zip(vals, vals[1:])]
                    if abs(total) > JUMP_PT and steps_:
                        big = max(steps_, key=abs)
                        moving = sum(1 for s in steps_ if abs(s) > 0.05)
                        if abs(big) > 0.5 * abs(total) or moving < 3:
                            if worst is None or abs(big) > abs(worst[1]):
                                worst = (label_c, big, total, moving)
                if worst:
                    c, big, total, moving = worst
                    out.append(dict(kind="snap", element=el.name, frames=moving, longest=moving, at=str(k0),
                                    detail=f"{label}: {c} covers {big:+.1f}pt of its {total:+.1f}pt move in one frame ({moving} moving frames)"))
                elif born and travel < 4 and h and h[0][5] >= POP_OPACITY:
                    out.append(dict(kind="snap", element=el.name, frames=1, longest=1, at=str(el.born),
                                    detail=f"{label}: appears at its place with no travel ({h[0][3]:.0f}×{h[0][4]:.0f} at {h[0][1]:.0f},{h[0][2]:.0f})"))
        if "leaves" in a["expect"]:
            key = e.get("element") or e.get("key", "")
            k0, k1 = window(a["end"], 1.2)
            el = find(key, k0, k1)
            if el is None:
                missing(a, key)
            else:
                def leaves(x):
                    if any(k0 <= k < k1 for k, _ in dying[x.id]):
                        return True
                    ops = [st[5] for st in x.hist if k0 - 1 <= st[0] < k1]
                    return bool(ops) and ops[0] > POP_OPACITY and ops[-1] <= VISIBLE_OPACITY
                gone = [x for x in els.values() if x.parent == el.parent and x.id != el.id and not x.flags & BLANK and leaves(x)]
                if not leaves(el) or gone:
                    who = ", ".join(short(x.name, 1) for x in gone[:3]) or "nothing"
                    out.append(dict(kind="wrong-exit", element=el.name, frames=1, longest=1, at=str(k0),
                                    detail=f"{label}: expected {short(el.name, 1)} to leave; {'it stays and ' if not leaves(el) else ''}{who} leave{'s' if len(gone) == 1 else ''}"
                                           + " (index keys?)"))

    # What an instant input changes is meant to cut: pops and jumps inside its window aren't findings.
    cuts = {"pop-in", "pop-out", "appear-then-jump", "jump", "step", "sibling-jump", "cut-exit"}

    def intended(f):
        if f["kind"] not in cuts:
            return False
        k = int(f["at"].split("–")[0]) if f["at"][:1].isdigit() else -1
        return any(k0 <= k < k1 + 1 and scope in f["element"] for k0, k1, scope in instant_windows)
    return [f for f in findings if not intended(f)] + out


def trace_name(path):
    name = path.rsplit("/", 1)[-1]
    return name[:-6] if name.endswith(".jsonl") else name


def allowed(rules, trace, f):
    """The allowlist rule that covers finding `f` in `trace`, if any."""
    for r in rules:
        kind = r.get("kind", f["kind"])
        if r.get("trace", trace) == trace and f["kind"] in ([kind] if isinstance(kind, str) else kind) \
                and r.get("element", "") in f["element"]:
            return r
    return None


def main():
    args = sys.argv[1:]
    out = allow_path = None
    quiet = "--quiet" in args
    fail = "--fail" in args
    args = [a for a in args if a not in ("--quiet", "--fail")]
    for flag in ("--json", "--allow"):
        if flag in args:
            i = args.index(flag)
            if flag == "--json":
                out = args[i + 1]
            else:
                allow_path = args[i + 1]
            del args[i:i + 2]
    jobs, script = [], None
    i = 0
    while i < len(args):
        if args[i] == "--script":
            with open(args[i + 1]) as f:
                script = json.load(f)
            i += 2
            continue
        jobs.append((args[i], script))
        script = None
        i += 1
    rules = []
    if allow_path:
        with open(allow_path) as f:
            rules = json.load(f)
        for r in rules:
            if not r.get("reason"):
                sys.exit(f"{allow_path}: every rule needs a reason: {r}")
    reports = [analyze(p, s) for p, s in jobs]
    used, open_ = set(), 0
    for r in reports:
        name = trace_name(r["trace"])
        for f in r["findings"]:
            rule = allowed(rules, name, f)
            f["allowed"] = rule["reason"] if rule else None
            if rule:
                used.add(id(rule))
        live = [f for f in r["findings"] if not f["allowed"]]
        open_ += len(live)
        print(f"== {name}: {r['frames']} frames, {r['animating_frames']} animating, {r['idle_frames']} idle, {len(live)} findings"
              + (f" (+{len(r['findings']) - len(live)} allowed)" if len(live) < len(r["findings"]) else ""))
        if quiet:
            continue
        for f in sorted(live, key=lambda f: (f["kind"], -f["frames"])):
            print(f"  {f['kind']:<16} {short(f['element']):<60} @{f['at']:<14} {f['detail']}")
    ran = {trace_name(r["trace"]) for r in reports}
    for r in rules:
        # Only a rule for one trace this run recorded can go stale; a general rule may wait.
        if id(r) not in used and r.get("trace") in ran:
            print(f"   unused allow rule: {json.dumps(r, ensure_ascii=False)}")
    if out:
        with open(out, "w") as f:
            json.dump(reports, f, indent=1)
    if fail:
        print(f"{open_} finding{'' if open_ == 1 else 's'} not in the allowlist" if open_ else "no findings outside the allowlist")
        sys.exit(1 if open_ else 0)


if __name__ == "__main__":
    main()
