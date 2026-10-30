"""Fake-bundle regressions: never launches a desktop application."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from mac_app_bundle import Bundle
from mac_app_install import launch, receipt_ready, parse_argv


class InstallerRegression(unittest.TestCase):
    def test_preserves_exact_arguments_without_reading_environment(self):
        data = (6).to_bytes(4, sys.byteorder) + b"/Applications/Demo.app/Contents/MacOS/demo\0\0demo\0--server\0https://example.com\0--directory\0/a path\0\0SECRET=not-an-argument\0"
        self.assertEqual(parse_argv(data), ["--server", "https://example.com", "--directory", "/a path", ""])

    def test_prebuilt_inactive_app_installs_without_launch_and_stamps_success(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            source = home / "source.app"
            binary = source / "Contents/MacOS/demo"
            binary.parent.mkdir(parents=True)
            binary.write_text("new")
            commands = home / "commands"
            commands.mkdir()
            for name, body in {"ps": "exit 0", "open": "exit 99", "codesign": "exit 99"}.items():
                path = commands / name
                path.write_text("#!/bin/sh\n" + body + "\n")
                path.chmod(0o755)
            stamp = home / "installed-rev"
            result = subprocess.run(
                [str(Path(__file__).with_name("mac-app")), "--name", "Demo",
                 "--id", "com.example.demo", "--exe", "demo", "--bundle", str(source),
                 "--only-relaunch", "--stamp", str(stamp), "--revision", "abc123"],
                env={**os.environ, "HOME": tmp, "PATH": f"{commands}:/usr/bin:/bin"},
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((home / "Applications/Demo.app/Contents/MacOS/demo").read_text(), "new")
            self.assertEqual(stamp.read_text(), "abc123\n")

    def bundle(self, home):
        live = Path(home) / "Demo.app"
        live.mkdir()
        (live / "version").write_text("old")
        return live

    def test_failed_launch_and_missing_receipt_roll_back(self):
        for reason in ("open failed", "ready receipt timed out"):
            with self.subTest(reason=reason), tempfile.TemporaryDirectory() as home:
                live = self.bundle(home)
                with Bundle(live) as bundle:
                    (bundle.stage / "version").write_text("new")

                    def start(rollback):
                        self.assertEqual((live / "version").read_text(), "old" if rollback else "new")
                        if not rollback:
                            raise RuntimeError(reason)

                    with self.assertRaisesRegex(RuntimeError, reason):
                        bundle.replace(lambda: None, start, lambda: None)
                self.assertEqual((live / "version").read_text(), "old")

    def test_unresponsive_replacement_still_restores_installed_path(self):
        with tempfile.TemporaryDirectory() as home:
            live = self.bundle(home)
            with Bundle(live) as bundle:
                (bundle.stage / "version").write_text("new")
                def fail(_rollback=False):
                    raise RuntimeError("unresponsive")
                with self.assertRaisesRegex(RuntimeError, "unresponsive"):
                    bundle.replace(lambda: None, fail, fail)
                self.assertEqual((live / "version").read_text(), "old")

    def test_receipt_timeout_uses_real_launch_path_and_rolls_back(self):
        with tempfile.TemporaryDirectory() as home:
            live = self.bundle(home)
            with Bundle(live) as bundle:
                (bundle.stage / "version").write_text("new")
                with patch("mac_app_install.command"), patch("mac_app_install.running", return_value={123: ()}), \
                     patch("mac_app_install.time.monotonic", side_effect=[0, 16]):
                    def start(rollback):
                        if not rollback:
                            launch(live, live / "demo", "com.example.demo", [], home)
                    with self.assertRaisesRegex(RuntimeError, "did not become ready"):
                        bundle.replace(lambda: None, start, lambda: None)
                self.assertEqual((live / "version").read_text(), "old")

    def test_interrupt_after_exchange_keeps_both_bundles(self):
        with tempfile.TemporaryDirectory() as home:
            live = self.bundle(home)
            script = """
from mac_app_bundle import Bundle
from pathlib import Path
import os, sys
with Bundle(sys.argv[1]) as bundle:
    (bundle.stage / 'version').write_text('new')
    (bundle.live.parent / 'backup-path').write_text(str(bundle.stage))
    bundle.publish()
    os._exit(0)
"""
            subprocess.run([sys.executable, "-c", script, str(live)], check=True,
                           cwd=Path(__file__).parent, timeout=10)
            self.assertEqual((live / "version").read_text(), "new")
            backup = Path((live.parent / "backup-path").read_text())
            self.assertEqual((backup / "version").read_text(), "old")
            with Bundle(live):  # no stale lock remains after death
                pass

    def test_termination_between_exchange_and_state_update_rolls_back(self):
        with tempfile.TemporaryDirectory() as home:
            live = self.bundle(home)
            script = """
import mac_app_bundle as m
import os, signal, sys
real_exchange = m.exchange
def interrupt(a, b):
    real_exchange(a, b)
    if (b / 'version').read_text() == 'new':
        os.kill(os.getpid(), signal.SIGTERM)
def terminated(*_):
    raise InterruptedError('terminated')
m.exchange = interrupt
signal.signal(signal.SIGTERM, terminated)
with m.Bundle(sys.argv[1]) as bundle:
    (bundle.stage / 'version').write_text('new')
    try:
        bundle.replace(lambda: None, lambda _: None, lambda: None)
    except InterruptedError:
        pass
    else:
        raise AssertionError('termination was lost')
    assert (bundle.live / 'version').read_text() == 'old'
"""
            subprocess.run([sys.executable, "-c", script, str(live)], check=True,
                           cwd=Path(__file__).parent, timeout=10)
            self.assertEqual((live / "version").read_text(), "old")

    def test_concurrent_installer_waits_until_rollback(self):
        with tempfile.TemporaryDirectory() as home:
            live = self.bundle(home)
            script = """
from mac_app_bundle import Bundle
import fcntl, pathlib, sys
live = pathlib.Path(sys.argv[1])
with (live.parent / f'.{live.name}.install.lock').open('a+b') as f:
    try:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        raise AssertionError('first installer has no lock')
    except BlockingIOError:
        print('blocked', flush=True)
with Bundle(live) as bundle:
    assert (live / 'version').read_text() == 'old'
    (bundle.stage / 'version').write_text('second')
    bundle.replace(lambda: None, lambda _: None, lambda: None)
"""
            with Bundle(live) as first:
                (first.stage / "version").write_text("first")
                first.publish()
                child = subprocess.Popen([sys.executable, "-c", script, str(live)],
                                         cwd=Path(__file__).parent, stdout=subprocess.PIPE, text=True)
                self.assertEqual(child.stdout.readline().strip(), "blocked")
                first.restore()
            try:
                self.assertEqual(child.wait(timeout=10), 0)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
                child.stdout.close()
            self.assertEqual((live / "version").read_text(), "second")

    def test_receipt_must_identify_replacement(self):
        with tempfile.TemporaryDirectory() as home:
            directory = Path(home)
            receipt = directory / "com.example.demo.123"
            receipt.write_text('{"pid": 124, "bundle_id": "com.example.demo"}')
            self.assertFalse(receipt_ready(directory, "com.example.demo", 123))
            receipt.write_text('{"pid": 123, "bundle_id": "com.example.demo"}')
            self.assertTrue(receipt_ready(directory, "com.example.demo", 123))

    def test_missing_stage_does_not_remove_live_app(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            live = home / "Applications/Demo.app"
            live.mkdir(parents=True)
            (live / "version").write_text("old")
            commands = home / "commands"
            commands.mkdir()
            # Simulate the other installer consuming this stage during codesign.
            for name, body in {
                "ps": "exit 0",
                "codesign": 'for arg; do last="$arg"; done; rm -rf "$last"',
                "open": "exit 1",
            }.items():
                path = commands / name
                path.write_text("#!/bin/sh\n" + body + "\n")
                path.chmod(0o755)
            binary = home / "demo"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
            stamp = home / "installed-rev"
            stamp.write_text("old-revision\n")
            result = subprocess.run(
                [str(Path(__file__).with_name("mac-app")), "--name", "Demo",
                 "--id", "com.example.demo", "--exe", "demo", "--bin", str(binary),
                 "--stamp", str(stamp), "--revision", "new-revision"],
                env={**os.environ, "HOME": tmp, "PATH": f"{commands}:/usr/bin:/bin", "MAC_APP_SIGN": ""},
                capture_output=True, text=True, timeout=10,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(live.is_dir(), result.stderr)
            self.assertEqual((live / "version").read_text(), "old")
            self.assertEqual(stamp.read_text(), "old-revision\n")


if __name__ == "__main__":
    unittest.main()
