"""Run on an authorized Mac, while the caller holds its GUI lock."""
import json, math, os, pathlib, re, selectors, socket, statistics, subprocess, sys, time
from prewarm_trace import assert_prewarmed

root = pathlib.Path(sys.argv[1])
retained = os.environ.get('OVERLAY_PROBE_RETAINED') == '1'
if subprocess.run(['pgrep', '-f', '[r]une-smoke/ci/Hex.app'], capture_output=True).returncode == 0:
    raise SystemExit('BLOCKED: competing Hex GUI probe is still alive')
control = root / 'control.sock'
env = dict(os.environ, OVERLAY_LAB_SECONDS='100', OVERLAY_LAB_CONTROL=str(control),
           DYLD_INSERT_LIBRARIES=str(root / 'presentation-probe.dylib'))
process = subprocess.Popen([str(root / 'overlay-lab')], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
selector = selectors.DefaultSelector()
selector.register(process.stdout, selectors.EVENT_READ)
buffer = b''
lines = []
log = (root / 'presentation.log').open('w')

def wait_for(predicate, seconds=20):
    global buffer
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        while b'\n' in buffer:
            raw, buffer = buffer.split(b'\n', 1)
            line = raw.decode(errors='replace')
            log.write(line + '\n'); log.flush(); lines.append(line)
            if predicate(line): return line
        if not selector.select(max(0, deadline - time.monotonic())): break
        chunk = os.read(process.stdout.fileno(), 65536)
        if not chunk: raise RuntimeError('overlay exited before expected event')
        buffer += chunk
    raise TimeoutError('native event did not arrive')

def command(value):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(str(control))
        connection.sendall((value+'\n').encode())
        reply = connection.recv(4096).decode().strip()
        print(reply, flush=True)
        return reply

try:
    created = wait_for(lambda line: 'pill created' in line)
    window = int(re.search(r'window_number: Some\((\d+)\)', created)[1])
    if retained:
        assert_prewarmed(lines, window)
    samples, identities, timed_out, alpha_ms, dropped = [], [], [], [], 0
    for trial in range(31):
        if trial:
            hidden = command('hide')
            assert f'window=Some({window})' in hidden, hidden
            prefix = f'probe alpha window={window} alpha=0.0 ' if retained else f'probe hidden window={window} visible=0 '
            wait_for(lambda line: line.startswith(prefix))
        state = subprocess.check_output([str(root / 'capture-probe'), str(window)] + ([] if retained else ['--hidden']), text=True, timeout=10).strip()
        assert 'exists=true' in state, state
        if not retained: assert 'onScreen=0' in state or 'onScreen=false' in state, state
        shown = command('show')
        assert f'window=Some({window})' in shown, shown
        requested = float(re.search(r'time=([\d.]+)', shown)[1])
        if retained:
            flipped = wait_for(lambda line: line.startswith(f'probe alpha window={window} alpha=1.0 '))
            alpha_ms.append((float(re.search(r'time=([\d.]+)', flipped)[1])-requested)*1000)
        def first_presentation(line):
            global dropped
            if not line.startswith(f'probe presented window={window} '): return False
            if float(re.search(r'acquired=([\d.]+)', line)[1]) < requested: return False
            presented = float(re.search(r'presented=([\d.]+)', line)[1])
            if presented == 0: dropped += 1
            return presented >= requested
        try:
            # Never hide after a discarded drawable. Wait for a positive timestamp;
            # a two-second absence is a failed trial, not a zero-latency success.
            frame = wait_for(first_presentation, seconds=2)
        except TimeoutError:
            timed_out.append(trial+1)
            delta = None
        else:
            presented = float(re.search(r'presented=([\d.]+)', frame)[1])
            assert presented >= requested, frame
            delta = (presented - requested) * 1000
            samples.append(delta)
        identities.append(window)
        print(f'trial={trial+1} show_to_present_ms={delta} window={window}', flush=True)
    capture = subprocess.run([str(root / 'capture-probe'), str(window), str(root / 'capture')], capture_output=True, text=True, timeout=20)
    print(capture.stdout, capture.stderr, flush=True)
    result = dict(samples_ms=samples, window_ids=identities, capture_exit=capture.returncode,
                  unpresented_callbacks=dropped, timed_out_trials=timed_out, alpha_ms=alpha_ms,
                  diagnostic_redraw=bool(os.environ.get('OVERLAY_PROBE_REDRAW')),
                  min_ms=min(samples) if samples else None, median_ms=statistics.median(samples) if samples else None,
                  p95_ms=sorted(samples)[math.ceil(len(samples)*.95)-1] if samples else None,
                  max_ms=max(samples) if samples else None, under_16=sum(v < 16 for v in samples))
    (root / 'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result), flush=True)
finally:
    # Ask the lab to quit; its bounded timer is a fallback if the socket is unavailable.
    try: command('quit')
    except (OSError, TimeoutError): pass
    process.wait(timeout=110)
    log.close()
    control.unlink(missing_ok=True)
