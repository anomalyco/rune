"""Regressions for check_rune_pins.py on small in-memory manifests and workspaces."""
import contextlib
import io
import tempfile
import tomllib
import unittest
from pathlib import Path

from check_rune_pins import check, main

URL = "https://github.com/anomalyco/rune"
MIRROR = "https://ghe.example.com/mirrors/rune"
REV = "60680797eff360a9ddbf68e72b45c85cfa8e35da"
RUNE = tomllib.loads("""
[patch.crates-io]
winit = { path = "vendor/winit" }
naga = { path = "vendor/naga" }
""")


def app(rune_url=URL, rev=REV, patches=("winit", "naga"), patch_url=None, patch_rev=None):
    lines = [f'[dependencies]\nrune = {{ git = "{rune_url}", rev = "{rev}" }}\n[patch.crates-io]']
    for name in patches:
        lines.append(f'{name} = {{ git = "{patch_url or rune_url}", rev = "{patch_rev or rev}" }}')
    return tomllib.loads("\n".join(lines))


def lock(*sources):
    packages = [f'[[package]]\nname = "{name}"\nversion = "{version}"\n' + (f'source = "{source}"\n' if source else "")
                for name, version, source in sources]
    return tomllib.loads("\n".join(packages))


def git(url=URL, commit=REV):
    return f"git+{url}?rev={commit}#{commit}"


class CheckRunePins(unittest.TestCase):
    def test_consistent_pins_pass(self):
        locked = lock(("rune", "0.0.0", git()), ("winit", "0.30.0", git()), ("wasm-bindgen", "0.2.129", None))
        self.assertEqual(check(app(), RUNE, locked, "0.2.129"), [])

    def test_mirror_url_on_every_line_passes(self):
        self.assertEqual(check(app(rune_url=MIRROR), RUNE, lock(("rune", "0.0.0", git(MIRROR)))), [])

    def test_patch_on_another_url_fails(self):
        problems = check(app(patch_url=MIRROR), RUNE)
        self.assertTrue(any("differ" in p for p in problems), problems)

    def test_patch_on_another_rev_fails(self):
        problems = check(app(patch_rev="f9882cf1c1664081cef8e5a5d6fa678fed800b97"), RUNE)
        self.assertTrue(any("differ" in p for p in problems), problems)

    def test_patch_list_drift_fails_both_ways(self):
        problems = check(app(patches=("winit", "sctk-adwaita")), RUNE)
        self.assertTrue(any("naga: missing" in p for p in problems), problems)
        self.assertTrue(any("sctk-adwaita: Rune no longer patches" in p for p in problems), problems)

    def test_two_locked_commits_fail(self):
        other = "f9882cf1c1664081cef8e5a5d6fa678fed800b97"
        problems = check(app(), RUNE, lock(("rune", "0.0.0", git()), ("winit", "0.30.0", git(commit=other))))
        self.assertTrue(any("more than one commit" in p for p in problems), problems)

    def test_wasm_bindgen_cli_mismatch_fails(self):
        problems = check(app(), RUNE, lock(("rune", "0.0.0", git()), ("wasm-bindgen", "0.2.129", None)), "0.2.128")
        self.assertTrue(any("wasm-bindgen" in p for p in problems), problems)

    def test_renamed_patch_key_passes(self):
        manifest = tomllib.loads(f"""
[dependencies]
rune = {{ git = "{URL}", rev = "{REV}" }}

[patch.crates-io]
rune-winit = {{ package = "winit", git = "{URL}", rev = "{REV}" }}
naga = {{ git = "{URL}", rev = "{REV}" }}
""")
        self.assertEqual(check(manifest, RUNE), [])

    def test_renamed_stale_patch_key_reports_package_name(self):
        manifest = tomllib.loads(f"""
[dependencies]
rune = {{ git = "{URL}", rev = "{REV}" }}

[patch.crates-io]
rune-winit = {{ package = "winit", git = "{URL}", rev = "{REV}" }}
naga = {{ git = "{URL}", rev = "{REV}" }}
rune-old = {{ package = "sctk-adwaita", git = "{URL}", rev = "{REV}" }}
""")
        problems = check(manifest, RUNE)
        self.assertTrue(any("sctk-adwaita: Rune no longer patches" in p for p in problems), problems)
        self.assertFalse(any("rune-winit" in p or "rune-old" in p for p in problems), problems)

    def test_split_url_same_commit_in_lock_fails(self):
        locked = lock(("rune", "0.0.0", git(URL, REV)), ("winit", "0.30.0", git(MIRROR, REV)))
        problems = check(app(rune_url=MIRROR), RUNE, locked)
        self.assertTrue(any("two copies of Rune" in p for p in problems), problems)
        self.assertTrue(any("more than one git URL" in p for p in problems), problems)

    def test_split_url_same_commit_rune_crates_in_lock_fails(self):
        locked = lock(("rune", "0.0.0", git(URL, REV)), ("rune-core", "0.0.0", git(MIRROR, REV)))
        problems = check(app(), RUNE, locked)
        self.assertTrue(any("two copies of Rune" in p for p in problems), problems)

    def test_split_url_same_commit_both_urls_in_manifest_lock_fails(self):
        locked = lock(("rune", "0.0.0", git(URL, REV)), ("winit", "0.30.0", git(MIRROR, REV)))
        problems = check(app(patch_url=MIRROR), RUNE, locked)
        self.assertTrue(any("more than one git URL" in p for p in problems), problems)

    def test_wrong_url_same_commit_in_lock_fails(self):
        locked = lock(("rune", "0.0.0", git(URL, REV)))
        problems = check(app(rune_url=MIRROR), RUNE, locked)
        self.assertTrue(any(URL in p and "git URL" in p for p in problems), problems)

    def test_unrelated_git_crate_in_lock_is_ignored(self):
        other_url = "https://github.com/example/other-crate"
        other_rev = "f9882cf1c1664081cef8e5a5d6fa678fed800b97"
        locked = lock(("rune", "0.0.0", git()), ("other-crate", "0.1.0", git(other_url, other_rev)))
        self.assertEqual(check(app(), RUNE, locked), [])

    def test_workspace_member_url_mismatch_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "rune-Cargo.toml").write_text(
                '[patch.crates-io]\nwinit = { path = "vendor/winit" }\nnaga = { path = "vendor/naga" }\n',
                encoding="utf-8",
            )
            (root / "Cargo.toml").write_text(
                f"""[workspace]
members = ["crates/*"]
exclude = ["crates/ignored"]

[patch.crates-io]
winit = {{ git = "{MIRROR}", rev = "{REV}" }}
naga = {{ git = "{MIRROR}", rev = "{REV}" }}
""",
                encoding="utf-8",
            )
            (root / "crates" / "app").mkdir(parents=True)
            (root / "crates" / "app" / "Cargo.toml").write_text(
                f'[package]\nname = "app"\nversion = "0.1.0"\n[dependencies]\nrune = {{ git = "{URL}", rev = "{REV}" }}\n',
                encoding="utf-8",
            )
            (root / "crates" / "ignored").mkdir(parents=True)
            (root / "crates" / "ignored" / "Cargo.toml").write_text(
                '[package]\nname = "ignored"\nversion = "0.1.0"\n[dependencies]\nrune = { git = "https://example.com/ignored/rune" }\n',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text(
                f"""[[package]]
name = "rune"
version = "0.0.0"
source = "{git(URL, REV)}"

[[package]]
name = "winit"
version = "0.30.0"
source = "{git(MIRROR, REV)}"
""",
                encoding="utf-8",
            )
            err = io.StringIO()
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = main([
                    "--rune-manifest", str(root / "rune-Cargo.toml"),
                    "--manifest", str(root / "Cargo.toml"),
                    "--lock", str(root / "Cargo.lock"),
                ])
            self.assertEqual(code, 1, f"stdout={out.getvalue()!r} stderr={err.getvalue()!r}")
            stderr = err.getvalue()
            self.assertIn("crates/app/Cargo.toml:dependencies.rune", stderr)
            self.assertIn("two copies of Rune", stderr)
            self.assertNotIn("crates/ignored", stderr)

    def test_workspace_members_globs_and_exclude_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "rune-Cargo.toml").write_text(
                '[patch.crates-io]\nwinit = { path = "vendor/winit" }\nnaga = { path = "vendor/naga" }\n',
                encoding="utf-8",
            )
            (root / "Cargo.toml").write_text(
                f"""[workspace]
members = [".", "crates/**"]
exclude = ["crates/ignored", "crates/skip-*"]

[patch.crates-io]
winit = {{ git = "{MIRROR}", rev = "{REV}" }}
naga = {{ git = "{MIRROR}", rev = "{REV}" }}
""",
                encoding="utf-8",
            )
            (root / "crates" / "app").mkdir(parents=True)
            (root / "crates" / "app" / "Cargo.toml").write_text(
                f'[package]\nname = "app"\nversion = "0.1.0"\n[dependencies]\nrune = {{ git = "{MIRROR}", rev = "{REV}" }}\n',
                encoding="utf-8",
            )
            # Nested inside excluded dir: must be excluded
            (root / "crates" / "ignored" / "nested").mkdir(parents=True)
            (root / "crates" / "ignored" / "nested" / "Cargo.toml").write_text(
                f'[package]\nname = "ignored"\nversion = "0.1.0"\n[dependencies]\nrune = {{ git = "{URL}" }}\n',
                encoding="utf-8",
            )
            # Excluded via glob: must be excluded
            (root / "crates" / "skip-fixture").mkdir(parents=True)
            (root / "crates" / "skip-fixture" / "Cargo.toml").write_text(
                f'[package]\nname = "skip"\nversion = "0.1.0"\n[dependencies]\nrune = {{ git = "{URL}" }}\n',
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text(
                f"""[[package]]
name = "rune"
version = "0.0.0"
source = "{git(MIRROR, REV)}"

[[package]]
name = "winit"
version = "0.30.0"
source = "{git(MIRROR, REV)}"
""",
                encoding="utf-8",
            )
            err = io.StringIO()
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = main([
                    "--rune-manifest", str(root / "rune-Cargo.toml"),
                    "--manifest", str(root / "Cargo.toml"),
                    "--lock", str(root / "Cargo.lock"),
                ])
            self.assertEqual(code, 0, f"stdout={out.getvalue()!r} stderr={err.getvalue()!r}")


if __name__ == "__main__":
    unittest.main()
