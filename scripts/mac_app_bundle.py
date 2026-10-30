"""Atomic bundle transaction. Lock protocol is also used by HiveMind land."""
import ctypes
import fcntl
import os
from pathlib import Path
import shutil
import signal
import sys
import tempfile


def exchange(a, b):
    libc = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "darwin":
        rename = libc.renamex_np
        rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        result = rename(os.fsencode(a), os.fsencode(b), 2)  # RENAME_SWAP
    else:
        rename = libc.renameat2
        rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        result = rename(-100, os.fsencode(a), -100, os.fsencode(b), 2)
    if result:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), str(a))


class Bundle:
    def __init__(self, live):
        self.live = Path(live).resolve()
        self.stage = None
        self.published = False
        self.committed = False

    def __enter__(self):
        self.live.parent.mkdir(parents=True, exist_ok=True)
        # Persistent inode, POSIX flock, same filename as HiveMind. Never unlink it.
        self.lock = (self.live.parent / f".{self.live.name}.install.lock").open("a+b")
        try:
            fcntl.flock(self.lock, fcntl.LOCK_EX)
            self.had_live = self.live.exists()
            self.stage = Path(tempfile.mkdtemp(prefix=f".{self.live.name}.install-", dir=self.live.parent))
        except BaseException:
            self.lock.close()
            raise
        return self

    def publish(self):
        # Deliver catchable termination only after the in-memory state matches the
        # syscall. SIGKILL still leaves both complete bundles on disk.
        previous = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT, signal.SIGTERM})
        try:
            if self.had_live:
                exchange(self.stage, self.live)
            else:
                os.rename(self.stage, self.live)
            self.published = True
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, previous)

    def restore(self):
        if not self.published:
            return
        if not self.had_live:
            raise RuntimeError("no previous bundle to restore")
        exchange(self.stage, self.live)
        self.published = False

    def replace(self, stop, launch, stop_replacement):
        """Keep the old bundle until launch and readiness succeed."""
        stop()
        try:
            self.publish()
            launch(False)
        except BaseException:
            try:
                stop_replacement()
            finally:
                # Restore the installed path even if the failed app refuses to quit.
                self.restore()
            launch(True)
            raise
        self.committed = True

    def __exit__(self, *_):
        try:
            if self.published and not self.committed:
                print(f"mac-app: previous bundle retained at {self.stage}", file=sys.stderr)
            elif self.stage and self.stage.exists():
                shutil.rmtree(self.stage)
        finally:
            self.lock.close()
