#!/usr/bin/env bash
# Checks folder shape for tracked source files under crates/ and examples/.
#   prefix family  x.rs or x/ plus 2+ x_*.rs siblings, or 3+ x_*.rs alone: error, "move into x/"
#   folder breadth > 40 non-test source files in one folder: warn; > 60: error
# Files directly in examples/, benches/ and bin/ are skipped (each is its own program).
# Test files (*_tests.rs, *_test.rs, tests.rs, *.test.ts, ...) are not counted. The prefix is the stem
# up to the first "_" or "-". Existing violations are listed in scripts/folder-shape-exceptions.txt
# ("<key> [ceiling]", key is "<dir>/<prefix>_*" or "<dir>/"; the ceiling is optional); ceilings only go down.
# Usage: scripts/folder-shape.sh [--warn-only] [--update-exceptions [--allow-raise]]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
exec python3 - "$@" <<'PY'
import os, re, subprocess, sys
from collections import defaultdict

WARN, HARD, FAM = 40, 60, 3
EXC = "scripts/folder-shape-exceptions.txt"
SRC = {".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs"}
PROGRAMS = {"examples", "benches", "bin"}  # each file is its own entry point, not a family
args = sys.argv[1:]
warn_only = "--warn-only" in args
update = "--update-exceptions" in args
raise_ok = "--allow-raise" in args
for a in args:
    if a not in ("--warn-only", "--update-exceptions", "--allow-raise"):
        sys.exit(f"unknown option: {a}")

files = subprocess.check_output(["git", "ls-files", "crates", "examples"], text=True).split("\n")
test_re = re.compile(r"(^|[_.-])tests?$")
dirs = defaultdict(lambda: ([], set()))  # dir -> (source stems, subdirs)
for f in files:
    if not f:
        continue
    d, name = os.path.split(f)
    parts = d.split("/")
    for i in range(2, len(parts)):  # register subdirs in their parents
        dirs["/".join(parts[:i])][1].add(parts[i])
    stem, ext = os.path.splitext(name)
    if parts[-1] in PROGRAMS:
        continue
    if ext not in SRC or test_re.search(stem) or "/tests/" in "/" + d + "/":
        continue
    dirs[d][0].append(stem)

found = {}  # key -> (count, hint)
for d, (stems, subs) in dirs.items():
    by = defaultdict(list)
    for s in stems:
        m = re.match(r"([^_-]+)[_-]", s)
        if m:
            by[m.group(1)].append(s)
    for p, group in by.items():
        base = p in stems or p in subs
        if len(group) >= (2 if base else FAM):
            found[f"{d}/{p}_*"] = (len(group), f"move into {p}/")
    if len(stems) > WARN:
        found[d + "/"] = (len(stems), "group into capability subfolders")

def load():
    out = {}
    if os.path.exists(EXC):
        for line in open(EXC):
            if line.strip() and not line.startswith("#"):
                parts = line.split()
                out[parts[0]] = int(parts[1]) if len(parts) > 1 else 10**9
    return out

exc = load()
if update:
    new = {}
    for k, (n, _) in found.items():
        if not (k.endswith("_*") or n > HARD):
            continue
        if k not in exc:
            if raise_ok:
                new[k] = n
            else:
                print(f"not listing new violation {k} ({n}); use --allow-raise", file=sys.stderr)
        else:
            new[k] = n if (n <= exc[k] or raise_ok) else exc[k]
    with open(EXC, "w") as fh:
        fh.write("# <key> <ceiling>: existing folder-shape violations (see scripts/folder-shape.sh). Ceilings only go down.\n")
        fh.write("# Refresh with scripts/folder-shape.sh --update-exceptions.\n")
        for k in sorted(new):
            fh.write(f"{k} {new[k]}\n")
    print(f"wrote {EXC} ({len(new)} entries)")
    sys.exit(0)

errors = warns = families = 0
gh = os.environ.get("GITHUB_ACTIONS") == "true"
rows = []
for k, (n, hint) in sorted(found.items()):
    fam = k.endswith("_*")
    families += fam
    is_err = fam or n > HARD
    if is_err:
        limit = exc.get(k)
        if limit is not None and n <= limit:
            warns += 1
            rows.append(f"{n:6d}  warn   {k} ({hint}; exception, ceiling {limit})")
        else:
            errors += 1
            rows.append(f"{n:6d}  ERROR  {k} ({hint}" + (f"; limit {limit}" if limit is not None else "") + ")")
            if gh:
                print(f"::error file={k}::{n} files: {hint}")
    else:
        warns += 1
        rows.append(f"{n:6d}  warn   {k} ({hint}, soft limit {WARN})")
print("\n".join(rows))
print(f"folder shape: families: {families}, folders over {WARN}: {sum(1 for k in found if not k.endswith('_*'))}, errors: {errors}, warnings: {warns}")
sys.exit(0 if warn_only or errors == 0 else 1)
PY
