"""Build, atomically install and check a Rune app before discarding its old bundle."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from mac_app_bundle import Bundle


def command(*args, check=True):
    return subprocess.run(args, check=check, capture_output=True, text=True, timeout=10)


def running(exe):
    result = {}
    for line in command("ps", "-axo", "pid=,lstart=,command=").stdout.splitlines():
        parts = line.split(None, 6)
        if len(parts) == 7 and (parts[6] == str(exe) or parts[6].startswith(str(exe) + " ")):
            result[int(parts[0])] = tuple(parts[1:6])
    return result


def parse_argv(data):
    argc = int.from_bytes(data[:4], sys.byteorder, signed=True)
    if argc <= 0:
        raise ValueError("invalid process argument count")
    rest = data[4:].split(b"\0", 1)[1].lstrip(b"\0")
    args = rest.split(b"\0")[:argc]
    if len(args) != argc:
        raise ValueError("truncated process arguments")
    return [os.fsdecode(arg) for arg in args[1:]]


def process_args(pid):
    libc = ctypes.CDLL(None, use_errno=True)
    mib = (ctypes.c_int * 3)(1, 49, pid)  # CTL_KERN, KERN_PROCARGS2
    data = ctypes.create_string_buffer(1024 * 1024)
    size = ctypes.c_size_t(len(data))
    if libc.sysctl(mib, 3, data, ctypes.byref(size), None, 0):
        raise OSError(ctypes.get_errno(), f"cannot preserve app pid {pid}'s arguments")
    return parse_argv(data.raw[:size.value])


def cocoa(pid, action):
    script = """ObjC.import('AppKit');
function run(a) {
  const app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(Number(a[0]));
  if (!app || app.isNil()) return 'gone';
  return (a[1] === 'quit' ? app.terminate : (!app.terminated && app.finishedLaunching)) ? 'yes' : 'no';
}"""
    return command("osascript", "-l", "JavaScript", "-e", script, str(pid), action).stdout.strip() == "yes"


def quit_apps(exe, apps):
    for pid, birth in apps.items():
        if running(exe).get(pid) == birth and not cocoa(pid, "quit"):
            if running(exe).get(pid) == birth:
                raise RuntimeError(f"app pid {pid} declined quit")
    deadline = time.monotonic() + 10
    while any(running(exe).get(pid) == birth for pid, birth in apps.items()):
        if time.monotonic() >= deadline:
            raise RuntimeError("app did not quit within 10s; no forced termination")
        time.sleep(0.1)


def receipt_ready(directory, bundle_id, pid):
    try:
        receipt = json.loads((directory / f"{bundle_id}.{pid}").read_text())
        return receipt["pid"] == pid and receipt["bundle_id"] == bundle_id
    except (OSError, ValueError, KeyError, TypeError):
        return False


def launch(live, exe, bundle_id, args, ready_root, require_ready=True):
    # A unique directory per launch excludes stale receipts, even after PID reuse.
    with tempfile.TemporaryDirectory(prefix="install-", dir=ready_root) as tmp:
        directory = Path(tmp)
        command("open", "-n", "--env", f"RUNE_READY_DIR={directory}", str(live), "--args", *args)
        deadline = time.monotonic() + 15
        while True:
            for pid in running(exe):
                if (not require_ready or receipt_ready(directory, bundle_id, pid)) and cocoa(pid, "ready"):
                    return
            if time.monotonic() >= deadline:
                raise RuntimeError("replacement did not become ready within 15s")
            time.sleep(0.1)


def wait_idle(bundle_id, seconds):
    if seconds is None:
        return
    while True:
        front = command("lsappinfo", "front").stdout.strip()
        info = command("lsappinfo", "info", "-only", "bundleid", front, check=False).stdout
        if f'"{bundle_id}"' not in info:
            return
        idle = command("ioreg", "-c", "IOHIDSystem").stdout
        match = re.search(r'"HIDIdleTime"\s*=\s*(\d+)', idle)
        if match and int(match[1]) / 1_000_000_000 >= seconds:
            return
        time.sleep(2)


def build(stage, args):
    if args.bundle:
        source = Path(args.bundle)
        if not (source / "Contents/MacOS" / args.exe).is_file():
            raise RuntimeError("source bundle has no executable")
        shutil.copytree(source, stage, dirs_exist_ok=True, symlinks=True)
        return
    macos = stage / "Contents/MacOS"
    resources = stage / "Contents/Resources"
    macos.mkdir(parents=True)
    resources.mkdir()
    shutil.copy2(args.bin, macos / args.exe)
    if args.icon:
        shutil.copy2(args.icon, resources / "icon.icns")
    plist = dict(CFBundleName=args.name, CFBundleDisplayName=args.name,
                 CFBundleIdentifier=args.id, CFBundleExecutable=args.exe,
                 CFBundleIconFile="icon", CFBundlePackageType="APPL",
                 CFBundleShortVersionString="0.0.1", LSMinimumSystemVersion="13.0",
                 NSHighResolutionCapable=True)
    if args.scheme:
        plist["CFBundleURLTypes"] = [dict(CFBundleURLName=args.id, CFBundleURLSchemes=[args.scheme])]
    if args.mic:
        plist["NSMicrophoneUsageDescription"] = args.mic
    with (stage / "Contents/Info.plist").open("wb") as file:
        plistlib.dump(plist, file)
    identity = os.environ.get("MAC_APP_SIGN") or "-"
    if command("codesign", "--force", "--sign", identity, str(stage), check=False).returncode:
        if identity == "-":
            raise RuntimeError("ad hoc signing failed")
        print("mac-app: signing identity failed; signing ad hoc", file=sys.stderr)
        command("codesign", "--force", "--sign", "-", str(stage))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("name", "id", "exe"):
        parser.add_argument(f"--{name}", required=True)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--bin")
    source.add_argument("--bundle", help="install an already signed bundle")
    for name in ("icon", "scheme", "mic"):
        parser.add_argument(f"--{name}")
    parser.add_argument("--wait-idle", type=float)
    parser.add_argument("--no-rune-ready", action="store_true", help="for binaries predating Rune readiness")
    parser.add_argument("--only-relaunch", action="store_true", help="do not open an app that was not running")
    parser.add_argument("--stamp", type=Path)
    parser.add_argument("--revision")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.args[:1] == ["--"]:
        args.args = args.args[1:]
    if args.bin and not os.access(args.bin, os.X_OK):
        parser.error(f"no executable binary at {args.bin}")
    if bool(args.stamp) != bool(args.revision):
        parser.error("--stamp and --revision must be used together")
    for name in (args.name, args.exe):
        if name in (".", "..") or "/" in name:
            parser.error("name and exe must be single path components")
    home = Path.home()
    live = home / "Applications" / f"{args.name}.app"
    exe = live / "Contents/MacOS" / args.exe
    ready_root = home / ".local/state/rune/ready"
    ready_root.mkdir(parents=True, exist_ok=True)
    with Bundle(live) as bundle:
        build(bundle.stage, args)
        wait_idle(args.id, args.wait_idle)
        busy = home / "Library/Caches/mac-app/busy"
        deadline = time.monotonic() + 900
        while any((busy / str(pid)).exists() for pid in running(exe)):
            if time.monotonic() >= deadline:
                raise RuntimeError("app still busy after 15 minutes")
            time.sleep(2)
        old = running(exe)
        launch_args = args.args
        if old and not launch_args:
            captured = [process_args(pid) for pid in old]
            if any(value != captured[0] for value in captured) or running(exe) != old:
                raise RuntimeError("running app instances changed or have different arguments")
            launch_args = captured[0]

        def start(rollback):
            if args.scheme:
                command("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
                        "-f", str(live), check=False)
            launch(live, exe, args.id, launch_args, ready_root,
                   require_ready=not (rollback or args.no_rune_ready))

        if args.only_relaunch and not old:
            bundle.publish()
            bundle.committed = True
        else:
            bundle.replace(lambda: quit_apps(exe, old), start,
                           lambda: quit_apps(exe, running(exe)))
        if args.stamp:
            stamp = args.stamp.expanduser()
            stamp.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(mode="w", dir=stamp.parent, delete=False) as file:
                file.write(args.revision + "\n")
                temporary = Path(file.name)
            try:
                os.replace(temporary, stamp)
            finally:
                temporary.unlink(missing_ok=True)
    print(live)


if __name__ == "__main__":
    def interrupted(signum, _frame):
        # A second termination signal must not interrupt the rollback itself.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        raise InterruptedError(f"installer interrupted by signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        main()
    except Exception as error:
        print(f"mac-app: {error}", file=sys.stderr)
        sys.exit(1)
