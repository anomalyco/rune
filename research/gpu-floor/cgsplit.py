#!/usr/bin/env python3
"""Split a callgrind profile's instructions into Rune, wgpu, wgpu-core, wgpu-hal, driver and
other, and count the driver entry points that non-driver code calls.

    cgsplit.py PROFILE [--frames N | --frame-fn SUBSTRING] [--top N]

A function belongs to a component by its crate (the first path segment of its demangled name)
or, for C code, by its object file. Code that belongs to no component (std, alloc, hashbrown,
libc, ...) is charged to its callers in proportion to the inclusive cost of each call edge, so
a `Vec` growth inside wgpu-core counts as wgpu-core. Per-frame numbers divide by `--frames`
or by the call count of the first function whose name contains `--frame-fn`.
"""

import argparse
import collections
import re
import sys

DRIVER_OBJECTS = ("libvulkan_lvp", "libgallium", "libLLVM", "libvulkan.so", "libvkd3d", "d3d12", "dxgi")
CRATES = {
    "wgpu_core": "wgpu-core",
    "wgpu_hal": "wgpu-hal",
    "ash": "wgpu-hal",
    "gpu_alloc": "wgpu-hal",
    "gpu_descriptor": "wgpu-hal",
    "wgpu": "wgpu",
    "wgpu_types": "wgpu",
    "naga": "wgpu-core",
    "floor": "probe",
    "wgpu_frame": "probe",
    "vk_floor": "probe",
    "dx12_floor": "probe",
}
COMPONENTS = ("rune", "probe", "wgpu", "wgpu-core", "wgpu-hal", "driver", "platform", "other")


def crate_of(name):
    s = name.lstrip("<&*")
    if s.startswith("mut "):
        s = s[4:]
    if s.startswith("dyn "):
        s = s[4:]
    m = re.match(r"([A-Za-z_][A-Za-z0-9_]*)::", s)
    return m.group(1) if m else None


def classify(name, obj):
    if any(d in obj for d in DRIVER_OBJECTS):
        return "driver"
    crate = crate_of(name)
    if crate is None:
        return None
    if crate in CRATES:
        return CRATES[crate]
    if crate.startswith("rune") or crate in ("overlay_hud_cost", "upload_bench", "layout_text_cost"):
        return "rune"
    if crate in ("winit", "x11rb", "x11_dl", "wayland_client", "wayland_backend", "calloop", "sctk", "smithay_client_toolkit"):
        return "platform"
    return None


def parse(path):
    names = {"fn": {}, "ob": {}}
    self_cost = collections.Counter()
    edges = collections.defaultdict(lambda: [0, 0])  # (caller, callee) -> [calls, inclusive]
    obj_of = {}
    ob = fn = cob = cfn = None
    pending_call = None

    def resolve(kind, rest):
        rest = rest.strip()
        m = re.match(r"\((\d+)\)(?:\s+(.*))?$", rest)
        if not m:
            return rest
        key, text = m.group(1), m.group(2)
        if text is not None:
            names[kind][key] = text
        return names[kind].get(key, key)

    with open(path, errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line:
                continue
            c = line[0]
            if c.isdigit() or c in "+-*":
                parts = line.split()
                cost = int(parts[1]) if len(parts) > 1 else 0
                if pending_call is not None:
                    e = edges[(fn, pending_call[0])]
                    e[0] += pending_call[1]
                    e[1] += cost
                    pending_call = None
                    cob = None
                else:
                    self_cost[fn] += cost
                continue
            if line.startswith("ob="):
                ob = resolve("ob", line[3:])
            elif line.startswith("fn="):
                fn = resolve("fn", line[3:])
                obj_of[fn] = ob
                cob = None
            elif line.startswith("cob="):
                cob = resolve("ob", line[4:])
            elif line.startswith("cfn="):
                cfn = resolve("fn", line[4:])
                obj_of.setdefault(cfn, cob or ob)
            elif line.startswith("calls="):
                pending_call = (cfn, int(line[6:].split()[0]))
    return self_cost, edges, obj_of


def attribute(self_cost, edges, obj_of):
    fns = set(self_cost) | {a for a, _ in edges} | {b for _, b in edges}
    cls = {f: classify(f or "", obj_of.get(f) or "") for f in fns}
    incoming = collections.defaultdict(list)
    for (a, b), (_, inc) in edges.items():
        if a != b:
            incoming[b].append((a, inc))
    dist = {f: ({cls[f]: 1.0} if cls[f] else {"other": 1.0}) for f in fns}
    for _ in range(40):
        changed = False
        for f in fns:
            if cls[f]:
                continue
            acc = collections.Counter()
            total = 0
            for a, inc in incoming.get(f, ()):
                if inc <= 0:
                    continue
                for k, v in dist[a].items():
                    acc[k] += inc * v
                total += inc
            if total:
                new = {k: v / total for k, v in acc.items()}
                if new != dist[f]:
                    dist[f] = new
                    changed = True
        if not changed:
            break
    out = collections.Counter()
    for f, c in self_cost.items():
        for k, v in dist[f].items():
            out[k] += c * v
    return out, cls, dist


def main():
    p = argparse.ArgumentParser()
    p.add_argument("profile")
    p.add_argument("--frames", type=float)
    p.add_argument("--frame-fn")
    p.add_argument("--top", type=int, default=40)
    p.add_argument("--calls", action="store_true", help="list per-frame calls of every wgpu-core and wgpu-hal entry")
    a = p.parse_args()
    self_cost, edges, obj_of = parse(a.profile)
    frames = a.frames
    if a.frame_fn:
        calls = collections.Counter()
        for (x, y), (n, _) in edges.items():
            if y and a.frame_fn in y:
                calls[y] += n
        if not calls:
            sys.exit(f"no function matches {a.frame_fn!r}")
        name, frames = calls.most_common(1)[0]
        print(f"frames: {frames} calls of {name}")
    frames = frames or 1
    split, cls, dist = attribute(self_cost, edges, obj_of)
    total = sum(split.values())
    print(f"total Ir: {total:,.0f}  per frame: {total / frames / 1e3:,.1f} k")
    for k in COMPONENTS:
        if split.get(k):
            print(f"  {k:10s} {split[k] / frames / 1e3:10.1f} k/frame  {100 * split[k] / total:5.1f}%")
    api = collections.Counter()
    api_cost = collections.Counter()
    for (x, y), (n, inc) in edges.items():
        if cls.get(y) == "driver" and dist.get(x, {}).get("driver", 0) < 0.5:
            api[y] += n
            api_cost[y] += inc
    print("driver entry points called from outside the driver (per frame, inclusive k):")
    for name, n in sorted(api.items(), key=lambda kv: -api_cost[kv[0]])[: a.top]:
        print(f"  {n / frames:8.2f}  {api_cost[name] / frames / 1e3:9.1f}k  {name}")
    allocs = collections.Counter()
    for (x, y), (n, _) in edges.items():
        if y in ("malloc", "calloc", "realloc", "aligned_alloc", "posix_memalign", "memalign"):
            for k, v in dist.get(x, {"other": 1.0}).items():
                allocs[k] += n * v
    print("heap allocations per frame by component: " + ", ".join(
        f"{k} {allocs[k] / frames:.1f}" for k in COMPONENTS if allocs.get(k, 0) / frames >= 0.05))
    wgpu = ("wgpu", "wgpu-core", "wgpu-hal")
    entry = collections.Counter()
    entry_calls = collections.Counter()
    for (x, y), (n, inc) in edges.items():
        cy = cls.get(y)
        if cy in wgpu and max(dist.get(x, {}), key=dist.get(x, {}).get, default=None) not in wgpu:
            entry[y] += inc
            entry_calls[y] += n
    print("wgpu entry points called from outside wgpu (per frame, inclusive k):")
    for name, inc in entry.most_common(a.top):
        if inc / frames >= 100:
            print(f"  {entry_calls[name] / frames:8.2f}  {inc / frames / 1e3:9.1f}k  {name[:150]}")


if __name__ == "__main__":
    main()
