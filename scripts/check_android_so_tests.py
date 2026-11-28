#!/usr/bin/env python3
"""Unit tests for scripts/check_android_so.py."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_android_so as mod


class CheckAndroidSoTests(unittest.TestCase):
    def test_parse_dt_needed_deduplicates_entries(self):
        readelf = """
Dynamic section at offset 0x1000 contains 5 entries:
  Tag                Type       Name/Value
 0x0000000000000001 (NEEDED)   Shared library: [libandroid.so]
 0x0000000000000001 (NEEDED)   Shared library: [liblog.so]
 0x0000000000000001 (NEEDED)   Shared library: [libc.so]
 0x0000000000000001 (NEEDED)   Shared library: [libc.so]
 0x000000000000000e (SONAME)   Library soname: [libandroid_hello.so]
"""
        self.assertEqual(mod.parse_dt_needed(readelf), ["libandroid.so", "liblog.so", "libc.so"])

    def test_parse_nm_distinguishes_strong_and_weak_undefined(self):
        nm_undef = """
                 U ANativeWindow_fromSurface
                 U android_main
                 U memcpy@LIBC
                 w __cxa_atexit@LIBC
                 w android_set_abort_message
"""
        self.assertEqual(
            mod.parse_nm_undefined(nm_undef),
            {"ANativeWindow_fromSurface", "android_main", "memcpy"},
        )

    def test_fails_when_android_main_is_undefined(self):
        so = Path("libandroid_hello.so")
        fake_outputs = {
            ("llvm-nm", "-D", "--defined-only", str(so)): """
0000000000010000 T JNI_OnLoad
0000000000010100 T ANativeActivity_onCreate
""",
            ("llvm-nm", "-D", "--undefined-only", str(so)): """
                 U ANativeWindow_fromSurface
                 U android_main
                 U memcpy@LIBC
                 w __cxa_atexit@LIBC
""",
            ("llvm-readelf", "-d", str(so)): """
 0x0000000000000001 (NEEDED) Shared library: [libandroid.so]
 0x0000000000000001 (NEEDED) Shared library: [libc.so]
""",
            ("llvm-nm", "-D", "--defined-only", str(Path("ndk/libandroid.so"))): """
0000000000000500 T ANativeWindow_fromSurface
""",
            ("llvm-readelf", "-d", str(Path("ndk/libandroid.so"))): "",
            ("llvm-nm", "-D", "--defined-only", str(Path("ndk/libc.so"))): """
0000000000000800 T memcpy@@LIBC
""",
            ("llvm-readelf", "-d", str(Path("ndk/libc.so"))): "",
        }
        errors, _, _ = mod.verify_android_so(
            so_path=so,
            clang="x86_64-linux-android35-clang",
            nm="llvm-nm",
            readelf="llvm-readelf",
            require_symbols=["JNI_OnLoad"],
            runner=lambda cmd: fake_outputs[tuple(cmd)],
            resolve_lib=lambda lib: Path(f"ndk/{lib}"),
        )
        self.assertEqual(
            errors,
            ["unresolved strong undefined symbol in libandroid_hello.so: android_main"],
        )

    def test_passes_when_android_main_is_defined_and_transitive_libs_resolve(self):
        so = Path("libandroid_hello.so")
        fake_outputs = {
            ("llvm-nm", "-D", "--defined-only", str(so)): """
0000000000010000 T JNI_OnLoad
0000000000010100 T ANativeActivity_onCreate
0000000000010200 T android_main
""",
            ("llvm-nm", "-D", "--undefined-only", str(so)): """
                 U ANativeWindow_fromSurface
                 U __android_log_write
                 U memcpy@LIBC
                 w __cxa_atexit@LIBC
""",
            ("llvm-readelf", "-d", str(so)): """
 0x0000000000000001 (NEEDED) Shared library: [libandroid.so]
 0x0000000000000001 (NEEDED) Shared library: [libc.so]
""",
            ("llvm-nm", "-D", "--defined-only", str(Path("ndk/libandroid.so"))): """
0000000000000500 T ANativeWindow_fromSurface
""",
            ("llvm-readelf", "-d", str(Path("ndk/libandroid.so"))): """
 0x0000000000000001 (NEEDED) Shared library: [liblog.so]
""",
            ("llvm-nm", "-D", "--defined-only", str(Path("ndk/liblog.so"))): """
0000000000000600 T __android_log_write
""",
            ("llvm-readelf", "-d", str(Path("ndk/liblog.so"))): "",
            ("llvm-nm", "-D", "--defined-only", str(Path("ndk/libc.so"))): """
0000000000000800 T memcpy@@LIBC
""",
            ("llvm-readelf", "-d", str(Path("ndk/libc.so"))): "",
        }
        errors, undef_count, lib_count = mod.verify_android_so(
            so_path=so,
            clang="x86_64-linux-android35-clang",
            nm="llvm-nm",
            readelf="llvm-readelf",
            require_symbols=["JNI_OnLoad"],
            runner=lambda cmd: fake_outputs[tuple(cmd)],
            resolve_lib=lambda lib: Path(f"ndk/{lib}"),
        )
        self.assertEqual(errors, [])
        self.assertEqual(undef_count, 3)
        self.assertEqual(lib_count, 3)

    def test_fails_when_required_export_or_dt_needed_missing(self):
        so = Path("libandroid_hello.so")
        fake_outputs = {
            ("llvm-nm", "-D", "--defined-only", str(so)): "",
            ("llvm-nm", "-D", "--undefined-only", str(so)): "",
            ("llvm-readelf", "-d", str(so)): """
 0x0000000000000001 (NEEDED) Shared library: [libmissing.so]
""",
        }
        errors, _, _ = mod.verify_android_so(
            so_path=so,
            clang="x86_64-linux-android35-clang",
            nm="llvm-nm",
            readelf="llvm-readelf",
            require_symbols=["JNI_OnLoad"],
            runner=lambda cmd: fake_outputs[tuple(cmd)],
            resolve_lib=lambda _lib: None,
        )
        self.assertEqual(
            errors,
            [
                "required exported symbol missing from libandroid_hello.so: JNI_OnLoad",
                "DT_NEEDED library not found in NDK sysroot: libmissing.so",
            ],
        )


if __name__ == "__main__":
    unittest.main()
