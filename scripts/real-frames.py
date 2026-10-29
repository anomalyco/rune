#!/usr/bin/env python3
"""Real-app frame costs from headless runs: per-frame distributions, launch and memory, and
where the main thread's time goes.

  run      Runs an app's scenarios from a JSON config, alternating their order across runs,
           with RUNE_FRAME_LOG set (build the app with rune/frame-profile). `--sample` adds one
           extra run per scenario under macOS `sample`, kept apart from the timed runs.
  summary  Reads a run directory: per scenario and window, frame CPU p50/p95/p99/max and frames
           over the 120 Hz budget (median across runs, warmup left out), phase means, memory.
  profile  Buckets a `sample` dump's main thread by self time (`--binary` adds file:line for
           the hottest functions through atos).

Config: {"binary": "...", "args": [...], "env": {...}, "fps": 120, "size": "1240x780",
         "scenarios": {"name": {"args": [...], "env": {...}, "frames": 600,
                                "script": [events] | "path", "windows": {"scroll": [1.0, 4.5]}}}}
Relative paths in the config resolve against the config's directory; "$OUT" in args and env
becomes the run directory.

Headless numbers are main-thread CPU for update, build, layout, paint and encode/submit. They
leave out GPU execution, presentation, display timing and the window server, and headless draws
every frame, so a frame "drawn" here may be one a real window skips: summary also reports the
frames a window would draw (input arrived, a commit, or something animating).
"""
import argparse
import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

BUDGET_MS = 1000 / 120


def load_config(path):
    cfg = json.loads(Path(path).read_text())
    cfg["_dir"] = Path(path).resolve().parent
    return cfg


def resolve(cfg, value, out):
    value = os.path.expandvars(str(value).replace("$OUT", str(out)))
    if value.startswith(("./", "../")):
        return str((cfg["_dir"] / value).resolve())
    return value


def command(cfg, name, sc, out, log):
    script = sc.get("script")
    if isinstance(script, list):
        path = out / f"{name}.script.json"
        path.write_text(json.dumps(script))
        script = str(path)
    elif script:
        script = resolve(cfg, script, out)
    args = [resolve(cfg, cfg["binary"], out)]
    args += [resolve(cfg, a, out) for a in cfg.get("args", []) + sc.get("args", [])]
    if script:
        args += ["--script", script]
    args += ["--fps", str(cfg.get("fps", 120)), "--frames", str(sc.get("frames", 600))]
    if cfg.get("size") and "--size" not in args:
        args += ["--size", cfg["size"]]
    env = dict(os.environ)
    env.update({k: resolve(cfg, v, out) for k, v in {**cfg.get("env", {}), **sc.get("env", {})}.items()})
    if log:
        env["RUNE_FRAME_LOG"] = str(log)
    cwd = resolve(cfg, sc.get("cwd", cfg.get("cwd", ".")), out)
    return args, env, cwd


def setup(cfg, sc, out):
    for step in cfg.get("setup", []) + sc.get("setup", []):
        subprocess.run(resolve(cfg, step, out), shell=True, check=True, cwd=cfg["_dir"])


def run(args):
    cfg = load_config(args.config)
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    names = [n for n in cfg["scenarios"] if not args.only or n in args.only.split(",")]
    failed = set()
    for n in range(args.runs + 1):
        order = names if n % 2 == 0 else names[::-1]
        for name in order:
            if name in failed:
                continue
            sc = cfg["scenarios"][name]
            setup(cfg, sc, out)
            log = out / f"{name}-{n}.jsonl"
            cmd, env, cwd = command(cfg, name, sc, out, log)
            start = time.monotonic()
            p = subprocess.run(cmd, env=env, cwd=cwd, capture_output=True, text=True, timeout=args.timeout)
            wall = time.monotonic() - start
            text = p.stdout + p.stderr
            (out / f"{name}-{n}.log").write_text(text)
            if p.returncode:
                # Keep going with the other scenarios; the log keeps the failure.
                print(f"{name} run {n} failed ({p.returncode}): {text[-1500:]}", file=sys.stderr, flush=True)
                failed.add(name)
                if log.exists():
                    log.rename(log.with_suffix(".failed"))
                continue
            launch = next((json.loads(l.split(" ", 1)[1]) for l in text.splitlines() if l.startswith("rune-first-pixels ")), None)
            with log.open("a") as f:
                f.write(json.dumps({"run": n, "warmup": n == 0, "process_wall_s": wall, "launch": launch}) + "\n")
            print(f"{name} run {n}: {wall:.2f} s", flush=True)
    if args.sample:
        for name in (n for n in names if n not in failed):
            sc = cfg["scenarios"][name]
            setup(cfg, sc, out)
            cmd, env, cwd = command(cfg, name, sc, out, None)
            # A still of the last frame, to check the scenario did what it says.
            if "--snap" not in cmd:
                cmd += ["--snap", str(out / f"{name}.check.png"), "--at", f"{(sc.get('frames', 600) - 1) / cfg.get('fps', 120):.3f}"]
            # The run holds before it starts (RUNE_HEADLESS_WAIT) so `sample` is attached for launch.
            env["RUNE_HEADLESS_WAIT"] = str(args.sample_hold)
            p = subprocess.Popen(cmd, env=env, cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            dump = out / f"{name}.sample.txt"
            subprocess.run(["/usr/bin/sample", str(p.pid), str(args.sample_secs), "1", "-mayDie", "-file", str(dump)],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            p.wait()
            print(f"{name}: sampled to {dump.name}", flush=True)


def q(values, p):
    v = sorted(values)
    return v[min(len(v) - 1, max(0, math.ceil(p * len(v)) - 1))] if v else None


def dist(values):
    return {"n": len(values), "p50": q(values, .5), "p95": q(values, .95), "p99": q(values, .99),
            "max": max(values, default=None), "over_budget": sum(v > BUDGET_MS for v in values),
            "missed_refreshes": sum(max(0, math.ceil(v / BUDGET_MS) - 1) for v in values)}


def read_log(path):
    frames, end, meta = [], None, None
    for line in path.read_text().splitlines():
        v = json.loads(line)
        if "k" in v:
            frames.append(v)
        elif v.get("end"):
            end = v
        elif "run" in v:
            meta = v
    return frames, end, meta


PHASES = ("view_cpu", "commit_cpu", "layout_cpu", "text_cpu", "paint_cpu", "apply_cpu", "update_ms", "encode_ms")


def window_stats(frames, fps, t0, t1):
    inside = [f for f in frames if t0 * fps <= f["k"] < t1 * fps]
    drawn = [f for f in inside if f["animating"] or f["commits"] or f["events"]]
    cost = lambda f: f["cpu_ms"] if f.get("cpu_ms") is not None else f["ms"]
    return {
        "all": dist([cost(f) for f in inside]),
        "drawn": dist([cost(f) for f in drawn]),
        "wall_drawn": dist([f["ms"] for f in drawn]),
        "phases_mean_ms": {p: statistics.fmean(f[p] for f in drawn) if drawn else None for p in PHASES},
        "allocations_mean": statistics.fmean(f["allocations"] for f in drawn) if drawn else None,
        "commits_per_frame": statistics.fmean(f["commits"] for f in inside) if inside else None,
        "shaped_mean": statistics.fmean(f["shaped"] for f in drawn) if drawn else None,
        "worst": sorted(({"k": f["k"], "t": round(f["k"] / fps, 3), "cpu_ms": cost(f), "shaped": f["shaped"],
                          "text_cpu": f["text_cpu"], "layout_cpu": f["layout_cpu"], "view_cpu": f["view_cpu"],
                          "encode_ms": f["encode_ms"], "allocations": f["allocations"]} for f in drawn),
                        key=lambda f: -f["cpu_ms"])[:5],
    }


def median_of(rows, path):
    vals = []
    for r in rows:
        v = r
        for key in path:
            v = v.get(key) if isinstance(v, dict) else None
        if v is not None:
            vals.append(v)
    return statistics.median(vals) if vals else None


def summary(args):
    cfg = load_config(args.config)
    out = Path(args.out)
    fps = cfg.get("fps", 120)
    report = {}
    for name, sc in cfg["scenarios"].items():
        logs = sorted(out.glob(f"{name}-*.jsonl"), key=lambda p: int(p.stem.rsplit("-", 1)[1]))
        if not logs:
            continue
        runs = []
        for path in logs:
            frames, end, meta = read_log(path)
            if not meta or meta.get("warmup"):
                continue
            windows = sc.get("windows") or {"all": [1 / fps, len(frames) / fps]}
            runs.append({"windows": {w: window_stats(frames, fps, *span) for w, span in windows.items()},
                         "memory": end, "launch": meta.get("launch"), "first_frame": frames[0] if frames else None,
                         "process_wall_s": meta.get("process_wall_s")})
        if not runs:
            continue
        entry = {"runs": len(runs), "windows": {}}
        for w in runs[0]["windows"]:
            ws = [r["windows"][w] for r in runs]
            entry["windows"][w] = {
                "drawn_cpu_ms": {k: median_of(ws, ["drawn", k]) for k in ("n", "p50", "p95", "p99", "max", "over_budget", "missed_refreshes")},
                "drawn_p50_range": [min(x["drawn"]["p50"] or 0 for x in ws), max(x["drawn"]["p50"] or 0 for x in ws)],
                "drawn_max_range": [min(x["drawn"]["max"] or 0 for x in ws), max(x["drawn"]["max"] or 0 for x in ws)],
                "all_cpu_ms": {k: median_of(ws, ["all", k]) for k in ("n", "p50", "p95", "p99", "max", "over_budget")},
                "phases_mean_ms": {p: median_of(ws, ["phases_mean_ms", p]) for p in PHASES},
                "allocations_mean": median_of(ws, ["allocations_mean"]),
                "commits_per_frame": median_of(ws, ["commits_per_frame"]),
                "shaped_mean": median_of(ws, ["shaped_mean"]),
                "worst_frames_by_run": [x["worst"][:3] for x in ws],
            }
        entry["memory_mb"] = {k: median_of(runs, ["memory", k]) / 1e6 for k in ("footprint", "footprint_peak", "resident", "heap_live")
                              if median_of(runs, ["memory", k]) is not None}
        if any(r["launch"] for r in runs):
            entry["launch_ms"] = {k: median_of(runs, ["launch", k]) for k in ("headless_ms", "state_ms", "runtime_ms", "gpu_init_ms", "frame_cpu_ms")}
        entry["first_frame_cpu_ms"] = median_of(runs, ["first_frame", "cpu_ms"])
        entry["first_frame_phases"] = {p: median_of(runs, ["first_frame", p]) for p in PHASES}
        report[name] = entry
    text = json.dumps(report, indent=1)
    if args.write:
        Path(args.write).write_text(text + "\n")
    print(table(report) if args.table else text)


def table(report):
    """One line per scenario window: drawn-frame CPU quantiles, budget misses and the phase split."""
    f = lambda v: "-" if v is None else f"{v:.2f}" if isinstance(v, float) else str(v)
    rows = ["scenario/window        n  p50   p95   p99   max    >8.3ms  view commit layout text paint update encode"]
    for name, e in report.items():
        for w, x in e["windows"].items():
            d, ph = x["drawn_cpu_ms"], x["phases_mean_ms"]
            rows.append(f"{name + '/' + w:<20} " + " ".join(f(d[k]) for k in ("n", "p50", "p95", "p99", "max", "over_budget"))
                        + "  " + " ".join(f(ph[k]) for k in ("view_cpu", "commit_cpu", "layout_cpu", "text_cpu", "paint_cpu", "update_ms", "encode_ms")))
        if e.get("launch_ms"):
            rows.append(f"{name}: launch " + json.dumps({k: round(v, 1) for k, v in e["launch_ms"].items() if v is not None})
                        + " first frame " + f(e["first_frame_cpu_ms"]) + " memory MB " + json.dumps({k: round(v) for k, v in e["memory_mb"].items()}))
    return "\n".join(rows)


# Buckets for self time, by the frame that owns it (generic std/alloc frames defer to their caller).
BUCKETS = [
    ("waiting (GPU, locks, sleep)", r"^(__semwait_signal|__psynch_cvwait|__psynch_mutexwait|mach_msg2_trap|mach_msg_trap|__workq_kernreturn|__ulock_wait|semaphore_wait_trap|nanosleep|kevent)"),
    ("allocation", r"^(_?malloc|_?free|_?realloc|_?calloc|szone_|tiny_|small_|nanov2_|_nanov2|mvm_|_malloc|_free|__rust_(alloc|dealloc|realloc)|alloc::alloc::|procwatch::alloc|<procwatch::alloc|xzm_|_xzm|malloc_|free_)"),
    ("motion", r"^(rune_motion::|<rune_motion::|rune::runtime::(motion|flights|transition|anim)|rune::motion|rune::anim)"),
    ("taffy", r"^(taffy::|<taffy::)"),
    # Walking laid-out glyphs, mostly from the paint walk's glyph emission rather than line breaking.
    ("glyph iteration (parley)", r"^<parley::layout::(run::Glyphs|line::GlyphRunIter|line::GlyphRun)\b"),
    ("text shaping (parley/harfrust/fontique)", r"^(parley::shape|<parley::shape|harfrust::|<harfrust::|harfruzz::|rustybuzz::|fontique::|<fontique::|skrifa::|<skrifa::|read_fonts::|<read_fonts::|swash::shape|parley::resolve|parley::builder|parley::context|parley::analysis|<parley::analysis|icu_|<icu_)"),
    ("text breaking/layout (parley)", r"^(parley::|<parley::)"),
    ("glyph raster/atlas", r"^(swash::|<swash::|zeno::|<zeno::|rune::text::(atlas|raster|glyph)|rune::glyph|etagere::|<etagere::|guillotiere::)"),
    ("rune text", r"^(rune::text|<rune::text|rune_kit::markdown|<rune_kit::markdown|pulldown_cmark|<pulldown_cmark|rune_kit::(code|syntax|highlight)|syntect|tree_sitter)"),
    ("flatten/diff/commit", r"^(rune::runtime::(flatten|commit|diff|reconcile|mount|keys|bands)|<rune::runtime::(flatten|commit)|rune::tree|rune::diff|rune::reconcile)"),
    ("rune layout (non-taffy)", r"^(rune::(runtime::)?(layout|measure)|<rune::(runtime::)?layout)"),
    ("rune render/encode", r"^(rune::(render|paint|renderer|batch|instances|gpu|scene|draw|shape|offscreen|record|surface)|<rune::(render|paint))"),
    ("wgpu/naga", r"^(wgpu|<wgpu|naga|<naga|metal::|<metal::|objc2_metal)"),
    ("driver/OS", r"^(objc_|_objc|-\[|\+\[|AGX|IOGPU|IOKit|IOConnect|mach_|__|_dispatch|dispatch_|CF|CA|MTL|_MTL|os_unfair|pthread_|_pthread|semaphore_|kevent|_platform_|swift_|AGXMetal|libsystem|dyld)"),
    # `Runtime::build` is the per-frame draw-list walk (Phase::Paint), not the view.
    ("rune paint walk", r"^(<rune::runtime::Runtime<.*>>::(build|glyphs_into|draw_text)\b|rune::runtime::paint)"),
    ("rune view/build", r"^(rune::|<rune::|rune_kit::|<rune_kit::|rune_theme|rune_fx|<rune_fx)"),
    ("hashing/containers (std)", r"^(hashbrown|<hashbrown|core::hash|<core::hash|std::collections|<std::collections|alloc::collections|<alloc::collections|rustc_hash|<rustc_hash|ahash|<ahash)"),
]
GENERIC = re.compile(r"^(<?(core|alloc|std|hashbrown|smallvec|<smallvec|rustc_hash|ahash)::|_platform_mem|DYLD-STUB|<&|<\[|<\(|<\*)")


def demangle(sym):
    s = sym
    for a, b in (("$LT$", "<"), ("$GT$", ">"), ("$u20$", " "), ("$RF$", "&"), ("$BP$", "*"), ("$C$", ","),
                 ("$u7b$", "{"), ("$u7d$", "}"), ("$u5b$", "["), ("$u5d$", "]"), ("$u27$", "'"), ("$u3b$", ";"),
                 ("$u22$", '"'), ("$LP$", "("), ("$RP$", ")"), ("$u2b$", "+"), ("$SP$", "@")):
        s = s.replace(a, b)
    s = s.replace("..", "::")
    s = re.sub(r"::h[0-9a-f]{16}$", "", s)
    return re.sub(r"^_(?=<)", "", s)


LINE = re.compile(r"^(?P<lead>[ +!:|]*)(?P<count>\d+) (?P<sym>.+?)  \(in (?P<lib>[^)]+)\)(?: \+ [\d,.]+)?"
                  r"(?:  \[(?P<addr>0x[0-9a-f]+)\](?:\s+(?P<src>\S+:\d+))?)?")


def demangle_all(syms):
    """Rust v0 and legacy symbols through rustfilt when it's installed, else the legacy rules."""
    syms = sorted(set(syms))
    if shutil.which("rustfilt"):
        p = subprocess.run(["rustfilt"], input="\n".join(syms), capture_output=True, text=True)
        out = p.stdout.splitlines()
        if p.returncode == 0 and len(out) == len(syms):
            return {a: re.sub(r"^_(?=<)", "", b) for a, b in zip(syms, out)}
    return {a: demangle(a) for a in syms}


def parse_threads(path):
    """{thread title: [(depth, count, sym, lib, addr)]} from a `sample` call graph."""
    threads, cur, in_graph = {}, None, False
    for line in Path(path).read_text().splitlines():
        if line.startswith("Call graph:"):
            in_graph = True
            continue
        if in_graph and (line.startswith("Total number in stack") or line.startswith("Sort by top of stack")):
            break
        if not in_graph or not line.strip():
            continue
        m = re.match(r"^ {4}(\d+) (Thread_\d+.*)$", line)
        if m:
            cur = m.group(2).strip()
            threads[cur] = []
            continue
        m = LINE.match(line[4:]) if cur else None
        if m:
            depth = len(m.group("lead")) // 2
            threads[cur].append((depth, int(m.group("count")), m.group("sym").strip(), m.group("lib"), m.group("addr"), m.group("src")))
    names = demangle_all(r[2] for rows in threads.values() for r in rows)
    return {t: [(d, c, names[sym], lib, addr, src) for d, c, sym, lib, addr, src in rows] for t, rows in threads.items()}


def bucket_of(sym, lib, app_libs):
    for name, rx in BUCKETS:
        if re.search(rx, sym):
            return name
    if lib in app_libs or re.match(r"^<?[a-z_0-9]+::", sym):
        return "app code"
    return "driver/OS" if lib not in app_libs else "other"


def self_times(rows):
    """(self count, symbol, owner symbol, lib, addr, source line) for each node of one thread's tree,
    leaving out the sampled run's RUNE_HEADLESS_WAIT hold."""
    children = [0] * len(rows)
    parents, stack = [None] * len(rows), []
    for i, (depth, count, *_rest) in enumerate(rows):
        while stack and rows[stack[-1]][0] >= depth:
            stack.pop()
        if stack:
            parents[i] = stack[-1]
            children[stack[-1]] += count
        stack.append(i)
    out = []
    for i, (depth, count, sym, lib, addr, src) in enumerate(rows):
        own = count - children[i]
        if own <= 0:
            continue
        path, j = [], i
        while j is not None:
            path.append(j)
            j = parents[j]
        if any(rows[j][2].startswith("std::thread::") and "sleep" in rows[j][2] for j in path):
            continue
        # The hold's sleep is inlined into headless_configured in optimized builds.
        if any(rows[j][2] == "nanosleep" and parents[j] is not None and "headless_configured" in rows[parents[j]][2] for j in path):
            continue
        owner = next((rows[j] for j in path if not GENERIC.match(rows[j][2]) or re.match(BUCKETS[0][1], rows[j][2])), rows[i])
        out.append((own, sym, owner[2], owner[3], addr, owner[5] or src))
    return out


def atos(binary, load, addrs):
    if not addrs:
        return {}
    p = subprocess.run(["atos", "-i", "-o", binary, "-l", load, *addrs], capture_output=True, text=True)
    lines, out, k = p.stdout.splitlines(), {}, 0
    # One block per address, inlined frames first, separated by blank lines.
    blocks, cur = [], []
    for line in lines:
        if not line.strip():
            if cur:
                blocks.append(cur)
            cur = []
        else:
            cur.append(line)
    if cur:
        blocks.append(cur)
    for addr, block in zip(addrs, blocks):
        out[addr] = block
    return out


def profile(args):
    text = Path(args.dump).read_text()
    threads = parse_threads(args.dump)
    if not any(threads.values()):
        sys.exit(f"{args.dump}: no samples (the process exited before sample attached)")
    main = next((t for t in threads if "main-thread" in t), None) or max(threads, key=lambda t: sum(c for d, c, *_ in threads[t] if d == 0))
    rows = threads[main]
    exe = re.search(r"^Path:\s+(.+)$", text, re.M)
    app_libs = {Path(exe.group(1)).name} if exe else set()
    nodes = self_times(rows)
    total = sum(n[0] for n in nodes)
    buckets, funcs = {}, {}
    for count, sym, owner, lib, addr, src in nodes:
        b = bucket_of(sym, lib, app_libs)
        if b in ("other", "driver/OS") or GENERIC.match(sym):
            ob = bucket_of(owner, lib, app_libs)
            b = b if b == "allocation" or (b == "driver/OS" and ob == "app code") else ob
        buckets[b] = buckets.get(b, 0) + count
        f = funcs.setdefault(owner, {"samples": 0, "bucket": b, "addrs": {}, "leaf": {}, "src": {}})
        f["samples"] += count
        if src:
            f["src"][src] = f["src"].get(src, 0) + count
        if addr:
            f["addrs"][addr] = f["addrs"].get(addr, 0) + count
        f["leaf"][sym] = f["leaf"].get(sym, 0) + count
    if args.exclude:
        rx = re.compile(args.exclude)
        idle = sum(c for c, s, *_ in nodes if rx.search(s))
    else:
        idle = 0
    top = sorted(funcs.items(), key=lambda kv: -kv[1]["samples"])[:args.top]
    lines = {}
    if args.binary:
        load = re.search(r"^Load Address:\s+(0x[0-9a-f]+)", text, re.M)
        addrs = [max(f["addrs"], key=f["addrs"].get) for _, f in top if f["addrs"]]
        lines = atos(args.binary, load.group(1), addrs) if load else {}
    report = {
        "thread": main, "samples": total, "ms_per_sample": 1,
        "buckets": {b: {"samples": c, "pct": round(100 * c / total, 1)} for b, c in sorted(buckets.items(), key=lambda kv: -kv[1])},
        "top": [{"fn": fn, "samples": f["samples"], "pct": round(100 * f["samples"] / total, 1), "bucket": f["bucket"],
                 "leaves": sorted(f["leaf"].items(), key=lambda kv: -kv[1])[:3],
                 "lines": sorted(f["src"].items(), key=lambda kv: -kv[1])[:3],
                 "at": lines.get(max(f["addrs"], key=f["addrs"].get)) if f["addrs"] else None} for fn, f in top],
    }
    text = json.dumps(report, indent=1)
    if args.write:
        Path(args.write).write_text(text + "\n")
    print(text)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("config")
    r.add_argument("--out", required=True)
    r.add_argument("--runs", type=int, default=5, help="measured runs, after one warmup")
    r.add_argument("--only", help="comma-separated scenarios")
    r.add_argument("--sample", action="store_true")
    r.add_argument("--sample-secs", type=int, default=120)
    r.add_argument("--sample-hold", type=float, default=1.5, help="seconds the sampled run waits before starting")
    r.add_argument("--timeout", type=int, default=600)
    s = sub.add_parser("summary")
    s.add_argument("config")
    s.add_argument("--out", required=True)
    s.add_argument("--write")
    s.add_argument("--table", action="store_true", help="print a one-line-per-window table instead of JSON")
    f = sub.add_parser("profile")
    f.add_argument("dump")
    f.add_argument("--binary")
    f.add_argument("--top", type=int, default=25)
    f.add_argument("--exclude")
    f.add_argument("--write")
    a = p.parse_args()
    {"run": run, "summary": summary, "profile": profile}[a.cmd](a)


if __name__ == "__main__":
    main()
