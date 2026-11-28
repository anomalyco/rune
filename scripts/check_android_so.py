#!/usr/bin/env python3
"""Verify that an Android shared library (.so) can be loaded by bionic's dynamic linker.

Checks that:
1. Every required exported symbol (for example `JNI_OnLoad`) is present in the library's
   defined dynamic symbol table.
2. Every `DT_NEEDED` shared library resolves in the Android NDK sysroot for the target clang.
3. Every strong undefined dynamic symbol (`U`) is provided by those `DT_NEEDED` libraries
   (weak undefined symbols `w` are ignored because bionic resolves missing weak symbols to null).

Usage:
    python3 scripts/check_android_so.py \
        --so target/x86_64-linux-android/debug/libandroid_hello.so \
        --clang "$CC_PATH" \
        --nm "$NDK_BIN/llvm-nm" \
        --readelf "$NDK_BIN/llvm-readelf" \
        --require-symbol JNI_OnLoad
"""

from __future__ import annotations

import argparse
from collections import deque
from pathlib import Path
import re
import subprocess
import sys
from typing import Callable, Iterable, Sequence

NEEDED_RE = re.compile(r"\(NEEDED\)\s+Shared library:\s+\[([^\]]+)\]")


def strip_symbol_version(symbol: str) -> str:
    """Strip ELF symbol version suffixes such as `@LIBC` or `@@LIBC`."""
    at = symbol.find("@")
    return symbol if at == -1 else symbol[:at]


def parse_dt_needed(readelf_output: str) -> list[str]:
    """Return all `DT_NEEDED` library basenames from `llvm-readelf -d` output."""
    libs: list[str] = []
    seen: set[str] = set()
    for line in readelf_output.splitlines():
        match = NEEDED_RE.search(line)
        if match:
            lib = match.group(1).strip()
            if lib and lib not in seen:
                seen.add(lib)
                libs.append(lib)
    return libs


def parse_nm_undefined(nm_output: str) -> set[str]:
    """Return strong undefined dynamic symbols (`U`) from `llvm-nm -D --undefined-only`."""
    symbols: set[str] = set()
    for line in nm_output.splitlines():
        parts = line.split()
        if len(parts) == 2:
            kind, name = parts
        elif len(parts) >= 3:
            kind, name = parts[1], parts[2]
        else:
            continue
        if kind == "U":
            symbols.add(strip_symbol_version(name))
    return symbols


def parse_nm_defined(nm_output: str) -> set[str]:
    """Return defined dynamic symbols from `llvm-nm -D --defined-only`."""
    symbols: set[str] = set()
    for line in nm_output.splitlines():
        parts = line.split()
        if len(parts) >= 3:
            kind, name = parts[1], parts[2]
        elif len(parts) == 2:
            kind, name = parts
        else:
            continue
        if kind not in ("U", "u", "w", "v"):
            symbols.add(strip_symbol_version(name))
    return symbols


def run_tool(cmd: Sequence[str]) -> str:
    proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or f"exit status {proc.returncode}"
        raise RuntimeError(f"command failed ({' '.join(cmd)}): {detail}")
    return proc.stdout


def resolve_ndk_lib(clang: str, lib_name: str, runner: Callable[[Sequence[str]], str]) -> Path | None:
    resolved = runner([clang, f"-print-file-name={lib_name}"]).strip()
    if not resolved or resolved == lib_name:
        return None
    path = Path(resolved)
    return path if path.is_file() else None


def verify_android_so(
    so_path: Path,
    clang: str,
    nm: str,
    readelf: str,
    require_symbols: Iterable[str] = (),
    runner: Callable[[Sequence[str]], str] = run_tool,
    resolve_lib: Callable[[str], Path | None] | None = None,
) -> tuple[list[str], int, int]:
    """Return `(errors, undefined_count, resolved_lib_count)` for `so_path`."""
    errors: list[str] = []

    defined_in_so = parse_nm_defined(runner([nm, "-D", "--defined-only", str(so_path)]))
    for req in require_symbols:
        if req not in defined_in_so:
            errors.append(f"required exported symbol missing from {so_path.name}: {req}")

    undefined = parse_nm_undefined(runner([nm, "-D", "--undefined-only", str(so_path)]))
    # Defined symbols in the .so itself are already satisfied even if also referenced.
    undefined -= defined_in_so

    needed_queue = deque(parse_dt_needed(runner([readelf, "-d", str(so_path)])))
    visited_libs: set[str] = set()
    provided: set[str] = set()
    resolved_count = 0

    resolver = resolve_lib or (lambda lib: resolve_ndk_lib(clang, lib, runner))

    while needed_queue:
        lib = needed_queue.popleft()
        if lib in visited_libs:
            continue
        visited_libs.add(lib)
        lib_path = resolver(lib)
        if lib_path is None:
            errors.append(f"DT_NEEDED library not found in NDK sysroot: {lib}")
            continue
        resolved_count += 1
        provided.update(parse_nm_defined(runner([nm, "-D", "--defined-only", str(lib_path)])))
        for transitive in parse_dt_needed(runner([readelf, "-d", str(lib_path)])):
            if transitive not in visited_libs:
                needed_queue.append(transitive)

    missing = sorted(undefined - provided)
    for sym in missing:
        errors.append(f"unresolved strong undefined symbol in {so_path.name}: {sym}")

    return errors, len(undefined), resolved_count


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--so", required=True, type=Path, help="Path to the built Android .so")
    parser.add_argument("--clang", required=True, help="Path to target NDK clang wrapper")
    parser.add_argument("--nm", required=True, help="Path to NDK llvm-nm")
    parser.add_argument("--readelf", required=True, help="Path to NDK llvm-readelf")
    parser.add_argument(
        "--require-symbol",
        action="append",
        default=[],
        help="Symbol that must be exported by the .so (repeatable)",
    )
    args = parser.parse_args(argv)

    if not args.so.is_file():
        print(f"error: shared library not found: {args.so}", file=sys.stderr)
        return 1

    try:
        errors, undef_count, lib_count = verify_android_so(
            so_path=args.so,
            clang=args.clang,
            nm=args.nm,
            readelf=args.readelf,
            require_symbols=args.require_symbol,
        )
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if errors:
        for err in errors:
            print(f"error: {err}", file=sys.stderr)
        return 1

    req_note = ""
    if args.require_symbol:
        req_note = f"; required exports present: {', '.join(args.require_symbol)}"
    print(
        f"Verified {args.so.name}: all {undef_count} strong undefined dynamic symbols "
        f"resolved across {lib_count} NDK DT_NEEDED libraries{req_note}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
