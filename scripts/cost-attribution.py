#!/usr/bin/env python3
"""Buckets the stacks `layout_text_cost --mode sample|heap` records by owner: Taffy, Parley
and its font stack, Rune (split by runtime phase), the app's view code, and the allocator.

  scripts/cost-attribution.py symbolize DUMP --exe BINARY > SYMS   # where BINARY was built
  scripts/cost-attribution.py report DUMP SYMS [--top 12]

`symbolize` runs `atos -i` (run `dsymutil BINARY` first) so inlined frames count. `report` needs
`rustfilt` on PATH to demangle. Exclusive attribution: a sample or allocation belongs to the
innermost frame that isn't the standard library, collections or memmove; a sample in malloc or
free is the allocator's, with its caller's owner noted. See research/31-layout-text-cost.md.
"""
import collections
import re
import subprocess
import sys

TRANSPARENT = {"core", "alloc", "std", "hashbrown", "smallvec", "arrayvec", "memchr", "compiler_builtins",
               "unicode_segmentation", "rustc_hash", "foldhash", "ahash", "bytemuck", "indexmap", "tinyvec", "slotmap",
               "usize", "isize", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32", "f64",
               "bool", "char", "str", "deduplicated_symbol", "DYLD", "?"}
OWNED = re.compile(r"\b(taffy|parley|parley_engine|fontique|swash|harfrust|skrifa|rune_kit|rune)::")
ALLOCATOR = re.compile(r"^(_?malloc|_?free|_?realloc|_?calloc|_?posix_memalign|_malloc_zone|_?szone|_?nanov2|_?mfm|_xzm|xzm|_?tiny_|_?small_|_?medium_|_?large_|_?default_zone|_?malloc_)")
MEMFN = re.compile(r"^_?(_platform_)?(memmove|memcpy|memset|bzero|memcmp|strlen|bcmp)")
PARLEY_PARTS = [  # first match on the stack, in this order
    ("Parley shaping (HarfRust, font tables)", re.compile(r"^<?(harfrust|read_fonts|skrifa|font_types)::|shape::shaper|shape_text")),
    ("Parley text analysis (ICU segmenter, bidi)", re.compile(r"^<?icu_|analysis::|itemize::|bidi::|analyze_text")),
    ("Parley layout clone (re-break)", re.compile(r"parley::layout::layout::Layout<.*> as core::clone::Clone>::clone")),
    ("Parley line breaking and alignment", re.compile(r"line_break|break_all_lines|alignment::align|BreakLines")),
]
PHASES = [  # innermost match wins
    ("layout", re.compile(r"text_layout::compute_layout")),
    ("flatten", re.compile(r"runtime::flatten::flatten")),
    ("view", re.compile(r">::(build_tree|root_view)$|rune::memo::")),
    ("place", re.compile(r">::place$")),
    ("final text pass", re.compile(r">::measure_view$")),
    ("apply", re.compile(r"runtime::Runtime<.*>>::apply$")),
    ("paint", re.compile(r"runtime::Runtime<.*>>::build$")),
]


def owner_of_crate(crate, name):
    if crate == "taffy":
        return "Taffy"
    if crate == "taffy" or "taffy::" in name and re.search(r"PartialEq|::eq\b", name):
        return "Taffy"
    if crate in ("parley", "parley_engine", "parley_data", "parley_emoji", "parlance", "harfrust", "skrifa",
                 "read_fonts", "font_types"):
        return "Parley"  # split by what the stack is doing, in `parley_part`
    if crate == "fontique":
        return "Fontique"
    if crate in ("swash", "zeno"):
        return "Swash raster"
    if crate.startswith("icu_") or crate in ("zerovec", "zerotrie", "yoke", "unicode_bidi", "unicode_script"):
        return "Parley"
    if crate == "rune":
        return "Instrumentation (frame-profile clock)" if "frame_profile" in name or "frame::profile" in name else "Rune"
    if crate in ("rune_kit", "pulldown_cmark"):
        return "Kit and Markdown"
    if crate == "layout_text_cost":
        return "App (bench views and data)"
    if crate in ("rune_motion",):
        return "Rune"
    return "Other: " + crate


def crate_of(name):
    """The crate a demangled frame belongs to; for `<T as Trait>::f` and drop glue, T's crate."""
    n = name
    m = re.match(r"^core::ptr::drop_in_place::<(.*)>$", n) or re.match(r"^core::ptr::drop_in_place<(.*)>$", n)
    if m:
        n = m.group(1)
    while n.startswith(("<", "&", "mut ", "dyn ", "[", "(", "*const ", "*mut ")):
        for p in ("<", "&", "mut ", "dyn ", "[", "(", "*const ", "*mut "):
            if n.startswith(p):
                n = n[len(p):]
    m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)", n)
    return m.group(1) if m else "?"


def demangle(names):
    names = list(names)
    out = subprocess.run(["rustfilt"], input="\n".join(names), capture_output=True, text=True, check=True).stdout.split("\n")
    return {a: re.sub(r"::h[0-9a-f]{16}$", "", b) for a, b in zip(names, out)}


def symbolize(dump, exe):
    addrs = collections.defaultdict(list)
    for line in open(dump):
        if line.startswith("ADDR "):
            _, a, base, sym, image = line.rstrip("\n").split(" ", 4)
            if image == exe or image.endswith("/" + exe.split("/")[-1]):
                addrs[base].append(a)
    for base, group in addrs.items():
        lst = "\n".join(f"0x{int(a, 16) - 1:x}" for a in group)
        res = subprocess.run(["atos", "-i", "-o", exe, "-l", "0x" + base], input=lst, capture_output=True, text=True, check=True).stdout
        blocks = res.split("\n\n")
        if len(blocks) < len(group):
            sys.exit(f"atos returned {len(blocks)} blocks for {len(group)} addresses")
        for a, block in zip(group, blocks):
            frames = [re.sub(r" \(in [^)]*\).*$", "", f).strip() for f in block.strip("\n").split("\n") if f.strip()]
            print("SYM " + a + " " + "\t".join(frames))


def load(dump, syms):
    cases, meta, addr, stacks = {}, [], {}, {}
    samples, allocs, live = [], [], []
    for line in open(dump):
        p = line.rstrip("\n").split(" ")
        if p[0] == "CASE":
            cases[int(p[1])] = p[2]
        elif p[0] == "META":
            meta.append(dict(kv.split("=", 1) for kv in p[1:]))
        elif p[0] == "ADDR":
            addr[p[1]] = [p[3]]  # system image: its exported symbol; replaced by SYMS below
        elif p[0] == "STACK":
            stacks[int(p[1])] = p[2:]
        elif p[0] == "SAMPLE":
            samples.append((int(p[1]), int(p[2]), int(p[3]), 0))
        elif p[0] == "ALLOC":
            allocs.append((int(p[1]), int(p[2]), int(p[3]), int(p[4])))
        elif p[0] == "LIVE":
            live.append((int(p[1]), int(p[2]), int(p[3]), int(p[4])))
    mangled = set()
    exe_frames = {}
    for line in open(syms):
        if line.startswith("SYM "):
            _, a, rest = line.rstrip("\n").split(" ", 2)
            exe_frames[a] = rest.split("\t")
            mangled.update(exe_frames[a])
    names = demangle(mangled)
    for a, frames in exe_frames.items():
        addr[a] = [names.get(f, f) for f in frames]
    return cases, meta, addr, stacks, samples, allocs, live


def classify(stack, addr, exe_addrs):
    """(owner, phase, allocator caller owner or None, inclusive owner set)."""
    frames = []  # (name, in_exe)
    for i, a in enumerate(stack):
        names = addr.get(a, ["?"])
        if i == 1 and len(frames) and a in exe_addrs and addr.get(stack[0]) == names:
            continue  # the link register pointing into pc's own function
        frames.extend((n, a in exe_addrs) for n in names)
    owner, alloc_owner, in_alloc, leaf = None, None, False, None
    inclusive = set()
    for name, in_exe in frames:
        if not in_exe:
            if owner is None and ALLOCATOR.match(name):
                in_alloc = True
            continue
        if re.search(r"layout_text_cost::probe|__rust_alloc|__rust_dealloc|__rust_realloc|__rdl_|__rg_", name):
            continue
        crate = crate_of(name)
        if crate in TRANSPARENT:
            # Dropping or cloning a container of another crate's values is that crate's.
            m = re.search(r"drop|clone", name) and OWNED.search(name)
            if not m:
                continue
            crate = m.group(1)
        o = owner_of_crate(crate, name)
        family = o.split(" ")[0]
        if family in ("Taffy", "Parley", "Fontique", "Swash"):
            inclusive.add("Parley+fonts" if family in ("Parley", "Fontique", "Swash") else family)
        if owner is None:
            owner, leaf = o, name
    phase = "other"
    for name, _ in frames:
        hit = next((p for p, rx in PHASES if rx.search(name)), None)
        if hit:
            phase = hit
            break
    if owner is None:
        owner = "System" if frames and not frames[0][1] else "Unknown"
    if owner == "Parley":
        owner = next((part for part, rx in PARLEY_PARTS if any(rx.search(n) for n, e in frames if e)), "Parley layout building")
    if in_alloc:
        alloc_owner, owner = owner, "Allocator"
    return owner, phase, alloc_owner, inclusive, leaf or (frames[0][0] if frames else "?")


def report(dump, syms, top):
    cases, meta, addr, stacks, samples, allocs, live = load(dump, syms)
    exe_addrs = set()
    for line in open(syms):
        if line.startswith("SYM "):
            exe_addrs.add(line.split(" ", 2)[1])
    cache = {}

    def cls(sid):
        if sid not in cache:
            cache[sid] = classify(stacks[sid], addr, exe_addrs)
        return cache[sid]

    for kind, rows in (("SAMPLES", samples), ("ALLOCS", allocs), ("LIVE", live)):
        if not rows:
            continue
        for tag, name in sorted(cases.items()):
            mine = [r for r in rows if r[0] == tag]
            if not mine:
                continue
            total = sum(r[2] for r in mine)
            total_b = sum(r[3] for r in mine)
            by_owner = collections.Counter()
            by_owner_b = collections.Counter()
            by_phase = collections.Counter()
            by_alloc = collections.Counter()
            incl = collections.Counter()
            funcs = collections.defaultdict(collections.Counter)
            funcs_b = collections.defaultdict(collections.Counter)
            for _, sid, n, b in mine:
                owner, phase, alloc_owner, inclusive, leaf = cls(sid)
                by_owner[owner] += n
                by_owner_b[owner] += b
                by_phase[(owner if not owner.startswith("Rune") else "Rune", phase)] += n
                if alloc_owner:
                    by_alloc[alloc_owner] += n
                for o in inclusive:
                    incl[o] += n
                funcs[owner][leaf[:140]] += n
                funcs_b[owner][leaf[:140]] += b
            print(f"\n== {kind} case={name} total={total}" + (f" bytes={total_b}" if kind != "SAMPLES" else ""))
            for o, n in by_owner.most_common():
                extra = f"  bytes {by_owner_b[o]:>12} {100 * by_owner_b[o] / max(total_b, 1):5.1f}%" if kind != "SAMPLES" else ""
                print(f"  {o:<34} {n:>9} {100 * n / total:5.1f}%{extra}")
            if kind == "SAMPLES":
                print("  -- inclusive (any frame on the stack)")
                for o, n in incl.most_common():
                    print(f"     {o:<31} {100 * n / total:5.1f}%")
                print("  -- by runtime phase (Rune = Rune's own code)")
                for (o, ph), n in sorted(by_phase.items(), key=lambda kv: -kv[1])[:24]:
                    print(f"     {o:<31} {ph:<16} {100 * n / total:5.1f}%")
                if by_alloc:
                    print("  -- allocator time by caller")
                    for o, n in by_alloc.most_common():
                        print(f"     {o:<31} {100 * n / total:5.1f}%")
            for o, _ in by_owner.most_common(6):
                print(f"  -- top in {o}")
                for f, n in funcs[o].most_common(top):
                    extra = f"  bytes {100 * funcs_b[o][f] / max(total_b, 1):5.1f}%" if kind != "SAMPLES" else ""
                    print(f"     {100 * n / total:5.1f}%{extra}  {f}")
    for m in meta:
        print("META", " ".join(f"{k}={v}" for k, v in m.items()))


if __name__ == "__main__":
    a = sys.argv[1:]
    if a and a[0] == "symbolize":
        symbolize(a[1], a[a.index("--exe") + 1])
    elif a and a[0] == "report":
        report(a[1], a[2], int(a[a.index("--top") + 1]) if "--top" in a else 12)
    else:
        sys.exit(__doc__)
