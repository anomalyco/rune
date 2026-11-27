#!/usr/bin/env python3
"""Check an app's Rune pins: one git URL and rev, Rune's patch list, one locked source.

Copy this file into an app that depends on Rune by git and run it from the app's workspace root
on every Rune bump (Python 3.11 or newer, no network, no other dependencies):

    git -C path/to/rune-clone show <rev>:Cargo.toml > rune-Cargo.toml
    python3 check_rune_pins.py --rune-manifest rune-Cargo.toml \
        [--manifest Cargo.toml] [--lock Cargo.lock] [--wasm-bindgen-cli 0.2.129]

Rune's root `Cargo.toml` at the pinned rev says which crates to patch. Every Rune git line across
the app's root manifest and `[workspace].members` (the `rune` dependency, any other Rune crate and
every `[patch.crates-io]` entry Rune lists) must use the same `git` URL and `rev`, the app must
patch exactly Rune's list, and `Cargo.lock` must resolve every Rune git source to one URL and
commit. `--wasm-bindgen-cli` compares the locked `wasm-bindgen` crate with the CLI version a CI
image or Dockerfile installs, as `scripts/web-build.sh` does. Prints each problem and exits 1 if
there is any.
"""
import argparse
import glob
from pathlib import Path
import sys
import tomllib

DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def norm_url(url):
    url = url.strip().rstrip("/")
    return url[:-4] if url.endswith(".git") else url


def git_lines(manifest, prefix=""):
    """Yield (where, crate, git url, rev) for every git dependency and patch in a manifest."""
    tables = []
    for name in DEP_TABLES:
        tables.append((name, manifest.get(name, {})))
    tables.append(("workspace.dependencies", manifest.get("workspace", {}).get("dependencies", {})))
    for cfg, target in manifest.get("target", {}).items():
        for name in DEP_TABLES:
            tables.append((f"target.{cfg}.{name}", target.get(name, {})))
    for source, entries in manifest.get("patch", {}).items():
        tables.append((f"patch.{source}", entries))
    for where, entries in tables:
        loc = f"{prefix}{where}" if prefix else where
        for key, spec in entries.items():
            if isinstance(spec, dict) and "git" in spec:
                yield loc, spec.get("package", key), norm_url(spec["git"]), spec.get("rev")


def is_rune_crate(name):
    return name in ("rune", "rune-ui") or name.startswith(("rune-", "rune_"))


def load_workspace_members(manifest_path, app):
    """Return {relative_manifest_path: parsed_toml} for `[workspace].members`, honoring `exclude`."""
    workspace = app.get("workspace", {})
    member_patterns = workspace.get("members", [])
    if not member_patterns:
        return {}
    root_dir = Path(manifest_path).resolve().parent
    excluded = set()
    for pattern in workspace.get("exclude", []):
        for match in glob.glob(pattern, root_dir=root_dir, recursive=True):
            excluded.add((root_dir / match).resolve())
    found = {}
    seen = set()
    for pattern in member_patterns:
        for match in glob.glob(pattern, root_dir=root_dir, recursive=True):
            member_dir = (root_dir / match).resolve()
            if member_dir == root_dir or not member_dir.is_dir():
                continue
            if any(member_dir == excl or excl in member_dir.parents for excl in excluded):
                continue
            member_manifest = member_dir / "Cargo.toml"
            if not member_manifest.is_file() or member_manifest in seen:
                continue
            seen.add(member_manifest)
            try:
                label = member_manifest.relative_to(root_dir).as_posix()
            except ValueError:
                label = str(member_manifest)
            with open(member_manifest, "rb") as f:
                found[label] = tomllib.load(f)
    return dict(sorted(found.items()))


def check(app, rune, lock=None, wasm_bindgen_cli=None, members=None):
    problems = []
    wanted = {
        spec.get("package", key) if isinstance(spec, dict) else key
        for key, spec in rune.get("patch", {}).get("crates-io", {}).items()
    }
    lines = list(git_lines(app))
    member_items = members.items() if isinstance(members, dict) else (members or ())
    for label, member in member_items:
        lines.extend(git_lines(member, f"{label}:"))
    urls = {url for _, name, url, _ in lines if is_rune_crate(name)}
    rune_lines = [
        line for line in lines
        if is_rune_crate(line[1]) or line[2] in urls
        or (line[0] == "patch.crates-io" and line[1] in wanted)
    ]
    if not rune_lines:
        return ["no Rune git dependency found in the manifest"]
    for where, name, url, rev in rune_lines:
        if not rev:
            problems.append(f"{where}.{name}: no `rev`; pin a `dev` rev")
    pins = {(url, rev) for _, _, url, rev in rune_lines}
    if len(pins) > 1:
        problems.append("Rune git lines differ; use one `git` URL and one `rev` on all of them:")
        problems += [f"  {where}.{name}: git = {url} rev = {rev}" for where, name, url, rev in rune_lines]
    patched = {name for where, name, _, _ in rune_lines if where == "patch.crates-io"}
    rune_urls = {u for _, _, u, _ in rune_lines}
    patched_keys = {
        spec.get("package", key)
        for key, spec in app.get("patch", {}).get("crates-io", {}).items()
        if isinstance(spec, dict) and "git" in spec and norm_url(spec["git"]) in rune_urls
    }
    patched |= patched_keys
    for name in sorted(wanted - patched):
        problems.append(f"patch.crates-io.{name}: missing; Rune patches it at this rev")
    for name in sorted(patched - wanted):
        problems.append(f"patch.crates-io.{name}: Rune no longer patches it at this rev; remove it")
    if lock is not None:
        problems += check_lock(lock, {u for u, _ in pins}, {r for _, r in pins if r}, wasm_bindgen_cli)
    return problems


def check_lock(lock, urls, revs, wasm_bindgen_cli):
    problems = []
    git_packages = []
    for package in lock.get("package", []):
        source = package.get("source", "")
        if not source.startswith("git+"):
            continue
        url, _, commit = source[4:].partition("#")
        git_packages.append((package["name"], norm_url(url.split("?", 1)[0]), commit))
    rune_lock_urls = set(urls) | {url for name, url, _ in git_packages if is_rune_crate(name)}
    sources = {}
    for name, url, commit in git_packages:
        if url in rune_lock_urls:
            sources.setdefault((url, commit), []).append(name)
    if not sources:
        problems.append("Cargo.lock: no package from the Rune git URL; run `cargo update -p rune`")
    elif len(sources) > 1:
        multi_url = len({u for u, _ in sources}) > 1
        multi_commit = len({c for _, c in sources}) > 1
        if multi_commit and multi_url:
            reason = "more than one commit and git URL"
        elif multi_commit:
            reason = "more than one commit"
        else:
            reason = "more than one git URL"
        problems.append(f"Cargo.lock: Rune's crates resolve to {reason} (two copies of Rune):")
        problems += [
            f"  {url}#{commit}: {', '.join(sorted(names))}"
            for (url, commit), names in sorted(sources.items())
        ]
    for url, commit in sorted(sources):
        if urls and url not in urls:
            problems.append(
                f"Cargo.lock: Rune is locked from {url}, not the manifest's git URL; run `cargo update -p rune`"
            )
        if revs and not any(commit.startswith(rev) for rev in revs):
            problems.append(f"Cargo.lock: Rune is locked at {commit}, not the manifest's rev; run `cargo update -p rune`")
    if wasm_bindgen_cli:
        locked = sorted({p["version"] for p in lock.get("package", []) if p["name"] == "wasm-bindgen"})
        if not locked:
            problems.append("Cargo.lock: no wasm-bindgen package to compare with --wasm-bindgen-cli")
        elif locked != [wasm_bindgen_cli]:
            problems.append(
                f"wasm-bindgen: Cargo.lock has {', '.join(locked)}, the CLI pin is {wasm_bindgen_cli}; "
                "make them match (scripts/web-build.sh refuses a mismatched CLI)"
            )
    return problems


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--rune-manifest", required=True, help="Rune's root Cargo.toml at the pinned rev")
    parser.add_argument("--manifest", default="Cargo.toml", help="the app's workspace root Cargo.toml")
    parser.add_argument("--lock", default="Cargo.lock", help="the app's Cargo.lock, or '' to skip it")
    parser.add_argument("--wasm-bindgen-cli", help="the wasm-bindgen-cli version CI installs")
    args = parser.parse_args(argv)
    with open(args.manifest, "rb") as f:
        app = tomllib.load(f)
    with open(args.rune_manifest, "rb") as f:
        rune = tomllib.load(f)
    lock = None
    if args.lock:
        with open(args.lock, "rb") as f:
            lock = tomllib.load(f)
    members = load_workspace_members(args.manifest, app)
    problems = check(app, rune, lock, args.wasm_bindgen_cli, members=members)
    for problem in problems:
        print(problem, file=sys.stderr)
    if not problems:
        print("Rune pins agree")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
