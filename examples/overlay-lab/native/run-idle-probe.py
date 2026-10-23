"""Measure normal-priority idle costs on an authorized Mac; caller holds its GUI lock."""
import ctypes, json, os, pathlib, selectors, socket, subprocess, sys, time

root = pathlib.Path(sys.argv[1])
control = root/'idle.sock'
env = dict(os.environ, OVERLAY_LAB_SECONDS='70', OVERLAY_LAB_CONTROL=str(control),
           DYLD_INSERT_LIBRARIES=str(root/'presentation-probe.dylib'))
env.pop('OVERLAY_PROBE_REDRAW', None)
env.pop('RUNE_OVERLAY_TRACE_INPUT', None)
log = (root/'idle.log').open('w')
process = subprocess.Popen([str(root/'overlay-lab')], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
selector = selectors.DefaultSelector()
selector.register(process.stdout, selectors.EVENT_READ)
buffer, frames = b'', 0

def drain(seconds):
    global buffer, frames
    deadline = time.monotonic()+seconds
    while True:
        while b'\n' in buffer:
            raw, buffer = buffer.split(b'\n',1)
            line = raw.decode(errors='replace'); log.write(line+'\n'); log.flush()
            if line.startswith('probe presented '): frames += 1
            if seconds == 0 and 'pill created' in line: return
        remaining = 10 if seconds == 0 else deadline-time.monotonic()
        if remaining <= 0: return
        if not selector.select(remaining):
            if seconds == 0: raise TimeoutError('no created callback')
            return
        chunk = os.read(process.stdout.fileno(),65536)
        if not chunk: raise RuntimeError('app exited')
        buffer += chunk

class Usage(ctypes.Structure):
    _fields_ = [('uuid',ctypes.c_uint8*16)] + [(name,ctypes.c_uint64) for name in
        ['user','system','package_wakes','interrupt_wakes','pageins','wired','resident','footprint','start','exit']]

lib = ctypes.CDLL('/usr/lib/libproc.dylib')
def usage():
    value = Usage()
    if lib.proc_pid_rusage(process.pid,0,ctypes.byref(value)) != 0: raise OSError('proc_pid_rusage failed')
    return value

def command(value):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5); client.connect(str(control)); client.sendall((value+'\n').encode())
        return client.recv(4096).decode()

results = []
try:
    drain(0)
    phases = [('hidden','status'),('visible','show'),('hidden-again','hide')]
    if '--modes' in sys.argv:
        command('show')
        phases = [('all','all'),('none','none'),('regions','regions')]
    for phase, action in phases:
        command(action)
        # Exclude transition work; drain logs continuously during the measurement.
        drain(1)
        before, count, started = usage(), frames, time.monotonic()
        drain(10)
        elapsed, after = time.monotonic()-started, usage()
        result = dict(phase=phase, seconds=elapsed,
            cpu_percent=(after.user+after.system-before.user-before.system)/1e9/elapsed*100,
            interrupt_wakes_per_second=(after.interrupt_wakes-before.interrupt_wakes)/elapsed,
            package_wakes_per_second=(after.package_wakes-before.package_wakes)/elapsed,
            drawable_callbacks=frames-count)
        results.append(result); print(json.dumps(result),flush=True)
    (root/'idle-results.json').write_text(json.dumps(results,indent=2))
finally:
    try: command('quit')
    except OSError: pass
    process.wait(timeout=75)
    log.close(); control.unlink(missing_ok=True)
