#!/usr/bin/env python3
"""Source-watch, external-crate and desk-relay regressions; no compiler, SSH, renderer or wall-clock waits."""
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

sys.dont_write_bytecode = True  # no __pycache__ beside the script
loader = importlib.machinery.SourceFileLoader("rune_dev", str(Path(__file__).with_name("rune-dev")))
spec = importlib.util.spec_from_loader(loader.name, loader)
dev = importlib.util.module_from_spec(spec)
loader.exec_module(dev)


class WatchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.names = ["Cargo.toml", "examples/settings/src/main.rs", "examples/settings/assets/font.ttf"]
        for name in self.names:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"original")
        for mock in [patch.object(dev, "ROOT", self.root), patch.object(dev, "files", lambda: self.names)]:
            mock.start()
            self.addCleanup(mock.stop)

    def fresh(self):
        return {p: (self.root / p).read_bytes() for p in self.names if (self.root / p).is_file()}

    def test_cache_matches_full_reads_through_saves_replacements_additions_and_deletions(self):
        cache = {}
        reads = []
        read = Path.read_bytes

        def counted(path):
            reads.append(path)
            return read(path)

        with patch.object(Path, "read_bytes", counted):
            first = dev.inputs(cache)
            self.assertEqual(len(reads), 3)
            self.assertEqual(dev.inputs(cache), first)
            self.assertEqual(len(reads), 3, "unchanged scans must not re-read assets or source")
            source = self.root / self.names[1]
            saved = source.stat()
            source.write_bytes(b"modified")
            os.utime(source, ns=(saved.st_atime_ns, saved.st_mtime_ns))
            changed = dev.inputs(cache)
            self.assertEqual(changed[self.names[1]], b"modified")
            self.assertEqual(reads[3:], [source], "same-size edits with restored mtime must be read")
            replacement = self.root / "replacement"
            replacement.write_bytes(b"replaced")
            os.utime(replacement, ns=(saved.st_atime_ns, saved.st_mtime_ns))
            replacement.replace(source)
            self.assertEqual(dev.inputs(cache)[self.names[1]], b"replaced")
        (self.root / self.names[0]).unlink()
        self.names.append("examples/settings/src/added.rs")
        (self.root / self.names[-1]).write_bytes(b"new")
        self.assertEqual(dev.inputs(cache), self.fresh())
        self.assertNotIn("Cargo.toml", cache)

    def test_partial_read_does_not_publish_a_new_cache(self):
        cache = {}
        original = dev.inputs(cache)
        source = self.root / self.names[1]
        source.write_bytes(b"edit")
        read = Path.read_bytes

        def racing(path):
            data = read(path)
            if path == source:
                path.write_bytes(b"finished edit")
            return data

        with patch.object(Path, "read_bytes", racing):
            with self.assertRaises(OSError):
                dev.inputs(cache)
        self.assertEqual({p: data for p, (_, data) in cache.items()}, original)
        self.assertEqual(dev.inputs(cache), self.fresh())

    def test_shared_watch_retries_read_races_and_reports_each_change_once(self):
        initial = {"view.rs": b"old"}
        edited = {"view.rs": b"new"}
        process = Mock()
        process.poll.side_effect = [None, None, None, None, None, 0]
        on_change, on_tick = Mock(), Mock()
        with patch.object(dev.select, "select"), patch.object(dev, "inputs", side_effect=[initial, OSError("save in progress"), edited, edited, {}]):
            dev.watch(on_change, process, initial, {}, on_tick)
        self.assertEqual([call.args[0] for call in on_change.call_args_list], [edited, {}])
        self.assertEqual(on_tick.call_count, 5)


class ArgumentTests(unittest.TestCase):
    def test_examples_keep_their_old_command_lines(self):
        opts = dev.parse(["settings", "--scene", "session", "--snap-on-patch", "a.png"])
        self.assertEqual((opts.example, opts.app_args), ("settings", ["--scene", "session", "--snap-on-patch", "a.png"]))
        self.assertEqual(dev.parse(["settings", "--", "--port", "1"]).port, 18744, "after -- every flag is the app's")

    def test_external_crates_name_their_patchable_files(self):
        opts = dev.parse(["--manifest-path", "../app/Cargo.toml", "--bin", "app", "--features", "fixture", "--patch", "src/ui/*.rs", "--desk", "--", "--hot-control", "c.json"])
        self.assertEqual((opts.manifest_path, opts.bin, opts.features, opts.patch, opts.desk), (Path("../app/Cargo.toml"), "app", "fixture", ["src/ui/*.rs"], True))
        self.assertEqual(opts.app_args, ["--hot-control", "c.json"])
        for bad in [[], ["settings", "--manifest-path", "x/Cargo.toml"], ["--manifest-path", "x/Cargo.toml"], ["settings", "--patch", "*.rs"]]:
            with self.subTest(bad=bad), self.assertRaises(SystemExit), patch("sys.stderr"):
                dev.parse(bad)

    def test_local_runs_take_the_flag_or_the_environment_but_not_desk(self):
        argv = ["--manifest-path", "app/Cargo.toml", "--patch", "src/view.rs"]
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("RUNE_DEV_LOCAL", None)
            self.assertFalse(dev.parse(argv).local)
            self.assertTrue(dev.parse([*argv, "--local"]).local)
            os.environ["RUNE_DEV_LOCAL"] = "1"
            self.assertTrue(dev.parse(argv).local)
        with self.assertRaises(SystemExit), patch("sys.stderr"):
            dev.parse([*argv, "--local", "--desk"])

    def test_a_local_run_rebuilds_after_a_restart_and_returns_the_apps_exit(self):
        with patch.dict(os.environ, {}, clear=False), patch.object(dev, "run", side_effect=[dev.Restart(), dev.Restart(), 3]) as run, patch("sys.stdout", io.StringIO()) as out:
            os.environ.pop("RUNE_DEV_WRAP", None)
            self.assertEqual(dev.local(["--local"]), 3)
        self.assertEqual(run.call_count, 3)
        self.assertEqual(out.getvalue().count("rune-dev: restarting with the new build"), 2)

    def test_a_local_run_execs_itself_once_under_the_wrap_prefix(self):
        with patch.dict(os.environ, {"RUNE_DEV_WRAP": "lease --kind build --"}, clear=False), patch.object(dev.os, "execvp") as execvp:
            os.environ.pop("RUNE_DEV_WRAPPED", None)
            execvp.side_effect = SystemExit(0)
            with self.assertRaises(SystemExit):
                dev.local(["--local", "--", "a"])
            command = execvp.call_args.args[1]
            self.assertEqual(command[:5], ["lease", "--kind", "build", "--", "python3"])
            self.assertEqual(command[-3:], ["--local", "--", "a"])
            self.assertEqual(os.environ["RUNE_DEV_WRAPPED"], "1")

    def test_remote_arguments_name_the_staged_crate_and_park_the_build_copy(self):
        opts = dev.parse(["--manifest-path", "/w/app/Cargo.toml", "--patch", "src/ui/**/*.rs", "--port", "9", "--", "--fixture", "f.json"])
        with patch.object(dev, "EXTERNAL", dev.External(Path("/w"), "external/app", ["app"], [])):
            self.assertEqual(dev.remote_argv(opts, False), ["--manifest-path", "external/app/Cargo.toml", "--patch", "src/ui/**/*.rs", "--port", "9", "--", "--fixture", "f.json"])
            self.assertEqual(dev.remote_argv(opts, True)[-1], "--desk")
            self.assertNotIn("f.json", dev.remote_argv(opts, True))

    def test_dx_gets_the_app_target_and_an_adhoc_profile(self):
        command = dev.dx_command("dx", 7, "app", "fixture,", "app", dev.PROFILE, [])
        self.assertEqual(command[command.index("--features") + 1], "hot,fixture")
        self.assertEqual(command[command.index("--bin") + 1], "app")
        cargo_args = next(a for a in command if a.startswith("--cargo-args="))
        self.assertIn("--config 'profile.rune-hot.inherits=\"dev\"'", cargo_args)
        self.assertFalse(any(a.startswith("--args") for a in command), "no app arguments: RUNE_HOT enables Rune")
        self.assertFalse(any(a.startswith("--cargo-args") for a in dev.dx_command("dx", 7, "app", "", None, [], ["--hot"])))


class ExternalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def write(self, name, text=""):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_layout_mirrors_path_dependencies_and_their_workspace_files(self):
        self.write("work/Cargo.toml", '[workspace]\nmembers = ["app", "ui"]\n[workspace.dependencies]\nshared = { path = "../shared" }\n')
        self.write("work/Cargo.lock")
        self.write("work/.cargo/config.toml")
        self.write("work/app/Cargo.toml", '[package]\nname = "app"\n[features]\nhot = ["rune/hot"]\n[dependencies]\nui = { path = "../ui" }\nshared.workspace = true\nrune = { git = "https://example.com/rune" }\n[lib]\npath = "src/lib.rs"\n')
        self.write("work/ui/Cargo.toml", '[package]\nname = "ui"\n[target.\'cfg(unix)\'.dev-dependencies]\nleaf = { path = "../../leaf" }\n')
        self.write("shared/Cargo.toml", '[package]\nname = "shared"\n')
        self.write("leaf/Cargo.toml", '[package]\nname = "leaf"\n')
        layout = dev.external_layout(self.root / "work/app/Cargo.toml")
        self.assertEqual(layout.base, self.root)
        self.assertEqual(layout.crate, "external/work/app")
        self.assertEqual(layout.dirs, ["leaf", "shared", "work/app", "work/ui"])
        self.assertEqual(sorted(layout.tops), ["work/.cargo/config.toml", "work/Cargo.lock", "work/Cargo.toml"])
        self.assertEqual(dev.crate_settings(self.root / "work/app", self.root), ("app", False))
        self.write("work/Cargo.toml", '[workspace]\n[profile.rune-hot]\ninherits = "dev"\n')
        self.assertEqual(dev.crate_settings(self.root / "work/app", self.root), ("app", True))
        self.write("work/ui/Cargo.toml", '[package]\nname = "ui"\n')
        with self.assertRaisesRegex(SystemExit, "hot"):
            dev.crate_settings(self.root / "work/ui", self.root)

    def test_dx_names_the_app_by_version_when_a_dependency_shares_its_name(self):
        self.write("work/Cargo.toml", '[workspace]\n[workspace.package]\nversion = "2.0.0"\n')
        self.write("work/app/Cargo.toml", '[package]\nname = "codec"\nversion.workspace = true\n')
        self.write("solo/Cargo.toml", '[package]\nname = "solo"\n')
        self.assertEqual(dev.package_spec(self.root / "work/app", self.root), "codec@2.0.0")
        self.assertEqual(dev.package_spec(self.root / "solo", self.root), "solo")
        self.write("real/cargo", "#!/bin/sh\nprintf '%s\\n' \"$@\"\n")
        (self.root / "real/cargo").chmod(0o755)
        path = dev.cargo_shim(self.root / "bin", "codec", "codec@2.0.0", str(self.root / "real"))
        self.assertEqual(path.split(os.pathsep)[0], str(self.root / "bin"))
        run = lambda *args: dev.subprocess.run(["cargo", *args], env={"PATH": path + os.pathsep + os.defpath}, capture_output=True, text=True, check=True).stdout.split()
        self.assertEqual(run("build", "-p", "codec", "--bin", "codec"), ["build", "-p", "codec@2.0.0", "--bin", "codec"])
        self.assertEqual(run("rustc", "--package=codec", "-pcodec"), ["rustc", "--package=codec@2.0.0", "-pcodec@2.0.0"])
        self.assertEqual(dev.cargo_shim(self.root / "none", "solo", "solo", str(self.root / "real")), str(self.root / "real"))

    def test_external_keys_read_the_original_files(self):
        self.write("app/Cargo.toml", '[package]\nname = "app"\n')
        self.write("app/src/ui/list.rs", "fn view() {}")
        self.write("app/target/debug/app", "binary")
        layout = dev.external_layout(self.root / "app/Cargo.toml")
        self.assertEqual(layout.crate, "external")
        with patch.object(dev, "EXTERNAL", layout):
            keys = dev.files()
            self.assertIn("external/src/ui/list.rs", keys)
            self.assertIn("scripts/rune-dev", keys)
            self.assertFalse(any("target/" in k for k in keys))
            self.assertEqual(dev.source("external/src/ui/list.rs").read_text(), "fn view() {}")
            self.assertEqual(dev.source("scripts/rune-dev"), dev.ROOT / "scripts/rune-dev")
            self.assertEqual(set(dev.inputs({})), {"external/Cargo.toml", "external/src/ui/list.rs"})

    def test_the_mirrored_workspace_lists_only_staged_members(self):
        manifest = ('[workspace]\nresolver = "2"\n# members below\nmembers = [\n  "app",  # the app\n  "crates/*",\n  "tools/[x]",\n]\n'
                    'default-members = ["app", "tools/cli"]\nexclude = ["crates/old"]\n\n[workspace.dependencies]\nui = { path = "crates/ui" }\n')
        self.write("ws/Cargo.toml", manifest)
        self.write("ws/app/Cargo.toml", '[package]\nname = "app"\n[features]\nhot = ["rune/hot"]\n[dependencies]\nui.workspace = true\n')
        self.write("ws/crates/ui/Cargo.toml", '[package]\nname = "ui"\n')
        self.write("ws/crates/old/Cargo.toml", '[package]\nname = "old"\n')
        self.write("ws/crates/other/Cargo.toml", '[package]\nname = "other"\n')
        self.write("ws/tools/cli/Cargo.toml", '[package]\nname = "cli"\n')
        layout = dev.external_layout(self.root / "ws/app/Cargo.toml")
        self.assertEqual(layout.dirs, ["app", "crates/ui"], "unrelated members aren't staged")
        with patch.object(dev, "EXTERNAL", layout):
            data = dev.inputs({})
        # The build Mac has only these: a workspace naming crates/other or tools/cli fails to load.
        self.assertNotIn("external/crates/other/Cargo.toml", data)
        overrides = dev.mirror_overrides(layout.crate, data)
        self.assertEqual(list(overrides), ["external/Cargo.toml"])
        staged = dev.tomllib.loads(overrides["external/Cargo.toml"].decode())
        self.assertEqual(staged["workspace"]["members"], ["app", "crates/ui"])
        self.assertEqual(staged["workspace"]["default-members"], ["app"])
        self.assertEqual(staged["workspace"]["dependencies"], {"ui": {"path": "crates/ui"}})
        self.assertIn("# members below", overrides["external/Cargo.toml"].decode(), "the rest of the file is kept")
        self.write("ws/Cargo.toml", '[workspace]\n')
        self.write("ws/app/Cargo.toml", '[package]\nname = "app"\nworkspace = ".."\n')
        self.assertEqual(dev.workspace_key("external/app", {"external/Cargo.toml": b"[workspace]\n", "external/app/Cargo.toml": b'[package]\nworkspace = ".."\n'}), "external/Cargo.toml")
        self.assertEqual(dev.mirror_overrides("external/app", {"external/app/Cargo.toml": b'[package]\nname = "app"\n'}), {}, "a lone crate is mirrored as is")

    def test_only_globbed_rust_files_in_the_app_crate_patch(self):
        allowed = dev.patchable("external/work/app", ["src/ui/*.rs", "src/views/**/*.rs"])
        self.assertTrue(allowed("external/work/app/src/ui/list.rs"))
        self.assertTrue(allowed("external/work/app/src/views/a/b/c.rs"))
        for key in ["external/work/app/src/ui/nested/list.rs", "external/work/app/src/ui/icon.svg", "external/work/app/src/main.rs",
                    "external/work/app/Cargo.toml", "external/work/ui/src/ui/list.rs", "external/work/application/src/ui/list.rs",
                    "external/rune/crates/rune/src/runtime.rs"]:
            self.assertFalse(allowed(key), key)


def ws_frame(opcode, payload, fin=True):
    return dev.frame(opcode, payload, fin)


class DeskTests(unittest.TestCase):
    def test_only_this_process_patch_is_fetched_and_rewritten(self):
        message = {"HotReload": {"jump_table": {"lib": "/build/libserver-patch-1.dylib", "map": {"4294983856": 4295000000}, "aslr_reference": 1, "new_base_address": 2},
                                 "templates": [], "assets": [], "ms_elapsed": 1, "for_pid": 42, "for_build_id": 0}}
        fetched = []
        def fetch(lib):
            fetched.append(lib)
            return Path("/desk/patches/libserver-patch-1.dylib")
        rewritten = json.loads(dev.rewrite_patch(json.dumps(message), 42, fetch))
        self.assertEqual(rewritten["HotReload"]["jump_table"]["lib"], "/desk/patches/libserver-patch-1.dylib")
        self.assertEqual(rewritten["HotReload"]["jump_table"]["map"], message["HotReload"]["jump_table"]["map"])
        self.assertEqual(fetched, ["/build/libserver-patch-1.dylib"])
        for other in [json.dumps(message).replace('"for_pid": 42', '"for_pid": 7'), '"HotPatchStart"', "not json", '{"HotReload": {"jump_table": null, "for_pid": 42}}']:
            self.assertEqual(dev.rewrite_patch(other, 42, fetch), other)
        self.assertEqual(len(fetched), 1)

    def test_a_patch_for_another_base_binary_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            desk = dev.Desk("build", Path(tmp), [], tmp, {})
            desk.remote_exe, desk.sha = "/build/server", "a" * 64
            result = Mock(returncode=0, stdout=b"b" * 64 + b"\nPATCH", stderr=b"")
            with patch.object(dev.subprocess, "run", return_value=result), self.assertRaisesRegex(RuntimeError, "not applying"):
                desk.fetch_patch("/build/libserver-patch-2.dylib")
            result.stdout = b"a" * 64 + b"\nPATCH"
            with patch.object(dev.subprocess, "run", return_value=result) as run:
                local = desk.fetch_patch("/build/libserver-patch-2.dylib")
            self.assertEqual(local.read_bytes(), b"PATCH")
            self.assertEqual(local.name, "libserver-patch-2.dylib")
            self.assertIn("shasum -a 256 /build/server", run.call_args.args[0][-1])

    def test_a_base_for_another_platform_fails_before_copying(self):
        local = {"system": "Darwin", "machine": "arm64"}
        self.assertIsNone(dev.check_base({"system": "Darwin", "machine": "arm64"}, local))
        self.assertIn("x86_64", dev.check_base({"system": "Darwin", "machine": "x86_64"}, local))
        with tempfile.TemporaryDirectory() as tmp, patch.object(dev, "platform_info", return_value=local), patch.object(dev.subprocess, "run") as run, patch("sys.stderr"):
            desk = dev.Desk("build", Path(tmp), [], tmp, {})
            desk.base({"exe": "/build/server", "sha256": "a" * 64, "system": "Linux", "machine": "arm64"})
        self.assertIn("Linux", desk.failed)
        run.assert_not_called()

    def test_a_base_report_behind_pty_bytes_still_starts_the_desk_app(self):
        info = {"exe": "/build/server", "sha256": "a" * 64, "system": "Darwin", "machine": "arm64"}
        for line in [dev.BASE + json.dumps(info) + "\n", "\r\x1b[K" + dev.BASE + json.dumps(info) + "\r\n"]:
            self.assertEqual(dev.base_message(line), info)
        self.assertIsNone(dev.base_message("Compiling app\n"))

    def forwarded(self, behaviour):
        """A stand-in for an `ssh -L` port: accepts, then answers or closes like a missing remote."""
        server = socket.create_server(("127.0.0.1", 0))
        self.addCleanup(server.close)
        def serve():
            while True:
                try:
                    conn, _ = server.accept()
                except OSError:
                    return
                with conn:
                    behaviour(conn)
        threading.Thread(target=serve, daemon=True).start()
        return server.getsockname()[1]

    def test_the_desk_app_waits_for_dx_through_the_tunnel_and_a_dead_tunnel_fails_loudly(self):
        silent = self.forwarded(lambda conn: None)
        self.assertFalse(dev.answers(silent), "ssh accepting a forward isn't dx answering")
        tunnel = Mock(returncode=255)
        tunnel.poll.side_effect = [None, None, 255]
        with self.assertRaisesRegex(RuntimeError, "tunnel to dx exited"):
            dev.wait_forward(silent, tunnel, timeout=30, interval=.01)
        with self.assertRaisesRegex(RuntimeError, "did not answer"):
            dev.wait_forward(silent, Mock(poll=Mock(return_value=None)), timeout=.05, interval=.01)
        calls = []
        def dx(conn):
            calls.append(1)
            if len(calls) > 2:
                conn.recv(1024)
                conn.sendall(b"HTTP/1.1 404 Not Found\r\n\r\n")
        dev.wait_forward(self.forwarded(dx), Mock(poll=Mock(return_value=None)), timeout=30, interval=.01)
        self.assertEqual(len(calls), 3, "the app starts once dx answers, not before")
        self.assertIsNone(dev.tunnel_failure(Mock(poll=Mock(return_value=None))))
        self.assertIn("Rerun", dev.tunnel_failure(Mock(poll=Mock(return_value=1), returncode=1)))

    def test_the_relay_retries_dx_while_the_forward_comes_up(self):
        calls = []
        def dx(conn):
            calls.append(1)
            if len(calls) < 3:
                return  # ssh closing a forward whose remote end isn't listening yet
            dev.read_head(conn)
            conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
            conn.recv(1)
        relay = dev.PatchRelay(("127.0.0.1", self.forwarded(dx)), lambda lib: lib, interval=.01)
        self.addCleanup(relay.close)
        with socket.create_connection(("127.0.0.1", relay.port), timeout=10) as app:
            app.sendall(b"GET /_dioxus?pid=7 HTTP/1.1\r\nHost: x\r\n\r\n")
            response, _ = dev.read_head(app)
        self.assertIn(b"101", response)
        self.assertEqual(len(calls), 3)

    def test_relay_rewrites_fragmented_patches_and_passes_other_frames(self):
        upstream = socket.create_server(("127.0.0.1", 0))
        self.addCleanup(upstream.close)
        patch_message = json.dumps({"HotReload": {"jump_table": {"lib": "/build/p.dylib", "map": {}}, "for_pid": 42}}).encode()
        long = json.dumps({"FullReloadStart": "x" * 70000}).encode()
        seen = {}
        def dx():
            conn, _ = upstream.accept()
            head, _ = dev.read_head(conn)
            seen["head"] = head
            conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
            conn.sendall(ws_frame(9, b"ping") + ws_frame(1, patch_message[:10], False) + ws_frame(0, patch_message[10:]) + ws_frame(1, long))
            fin, op, payload = dev.read_frame(conn.makefile("rb"))
            seen["client"] = (fin, op, payload)
            conn.close()
        server = threading.Thread(target=dx, daemon=True)
        server.start()
        relay = dev.PatchRelay(upstream.getsockname(), lambda lib: Path("/desk/p.dylib"))
        self.addCleanup(relay.close)
        with socket.create_connection(("127.0.0.1", relay.port)) as app:
            app.sendall(b"GET /_dioxus?aslr_reference=1&build_id=0&pid=42 HTTP/1.1\r\nHost: x\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n")
            response, rest = dev.read_head(app)
            self.assertIn(b"101", response)
            data = rest
            def next_frame():
                nonlocal data
                while True:
                    try:
                        frame = dev.read_frame(io.BytesIO(data))
                        size = len(dev.frame(frame[1], frame[2], frame[0]))
                        data = data[size:]
                        return frame
                    except EOFError:
                        chunk = app.recv(65536)
                        if not chunk: raise
                        data += chunk
            frames = [next_frame() for _ in range(3)]
            mask = b"\x01\x02\x03\x04"
            payload = b"pong"
            app.sendall(bytes([0x8a, 0x80 | len(payload)]) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))
            server.join(5)
        self.assertNotIn(b"permessage-deflate", seen["head"].lower())
        self.assertEqual(frames[0], (True, 9, b"ping"))
        self.assertEqual(json.loads(frames[1][2])["HotReload"]["jump_table"]["lib"], "/desk/p.dylib")
        self.assertEqual(frames[2], (True, 1, long))
        self.assertEqual(seen["client"], (True, 10, b"pong"), "the app's frames reach dx unchanged")


if __name__ == "__main__":
    unittest.main()
