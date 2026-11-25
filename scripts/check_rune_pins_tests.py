"""Regressions for check_rune_pins.py on small in-memory manifests."""
import tomllib
import unittest

from check_rune_pins import check

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


if __name__ == "__main__":
    unittest.main()
