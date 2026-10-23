"""Explicitly authorized native input test; caller holds the host GUI lock."""
import ast, json, os, pathlib, re, selectors, socket, subprocess, sys, time

root = pathlib.Path(sys.argv[1])
control = root / 'input.sock'
env = dict(os.environ, OVERLAY_LAB_SECONDS='90', RUNE_OVERLAY_TRACE_INPUT='1',
           OVERLAY_LAB_INCLUDE_IN_CAPTURE='1')
underlay = subprocess.Popen([str(root/'overlay-lab')], env=dict(env, OVERLAY_LAB_UNDERLAY='1'),
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
overlay = subprocess.Popen([str(root/'overlay-lab')], env=dict(env, OVERLAY_LAB_CONTROL=str(control)),
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
selector = selectors.DefaultSelector()
for name, process in [('underlay', underlay), ('overlay', overlay)]:
    selector.register(process.stdout, selectors.EVENT_READ, name)
buffers = {'underlay': b'', 'overlay': b''}
logs = {name: (root/f'input-{name}.log').open('w') for name in buffers}
regions, clicks, chips, window = [], 0, 0, None
snapshots = {}

def pump(until, seconds=5):
    global regions, clicks, chips, window
    deadline = time.monotonic() + seconds
    while not until():
        if time.monotonic() >= deadline: raise TimeoutError('input assertion timed out')
        for key, _ in selector.select(max(0, deadline-time.monotonic())):
            name = key.data
            chunk = os.read(key.fileobj.fileno(), 65536)
            if not chunk: raise RuntimeError(f'{name} exited')
            buffers[name] += chunk
            while b'\n' in buffers[name]:
                line, buffers[name] = buffers[name].split(b'\n', 1)
                line = line.decode(errors='replace')
                logs[name].write(line+'\n'); logs[name].flush()
                if name == 'underlay' and 'underlay click ' in line:
                    clicks = int(line.rsplit(' ', 1)[1])
                if name == 'overlay' and 'overlay-lab chip iPhone' in line: chips += 1
                if name == 'overlay' and 'pill created' in line:
                    window = int(re.search(r'window_number: Some\((\d+)\)', line)[1])
                if line.startswith('overlay-input ') and 'regions=' in line:
                    observed = int(re.search(r'window=(\d+)', line)[1])
                    snapshots[observed] = ast.literal_eval(line.split('regions=', 1)[1])
                regions = snapshots.get(window, [])

def command(value):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5); client.connect(str(control)); client.sendall((value+'\n').encode())
        return client.recv(4096).decode()

def move(start, finish, click=False):
    helper = os.environ.get('OVERLAY_MOUSE_HELPER', str(pathlib.Path.home()/'tmp/rune-overlay-probe/mouse-path'))
    result = subprocess.run([helper, *map(str, start+finish), '32'] + (['click'] if click else []),
                            capture_output=True, text=True, timeout=5)
    print(result.stdout.strip(), flush=True)
    if result.returncode: raise RuntimeError(f'input helper failed: {result.returncode} {result.stderr}')

try:
    pump(lambda: window is not None and len(regions) >= 4)
    command('show')
    geometry = json.loads(subprocess.check_output([str(root/'capture-probe'), str(window), '--geometry'], text=True))
    x, y = geometry['X'], geometry['Y']
    outside = (x+20, y+80)
    move((x+20, y+200), outside)
    for trial in range(20):
        # iPhone is the fourth hit chip; its geometry shifts when Route appears.
        r = regions[3]
        inside = (x+(r[0]+r[2])/2, y+(r[1]+r[3])/2)
        move(outside, inside, True)
        pump(lambda: chips == trial+1 or clicks > trial)
        assert chips == trial+1 and clicks == trial, f'inside: chips={chips} underlay={clicks} trial={trial+1}'
        move(inside, outside, True)
        pump(lambda: clicks == trial+1)
        assert chips == trial+1 and clicks == trial+1
        print(f'PASS cycle={trial+1} overlay={chips} underlay={clicks}', flush=True)
    (root/'input-results.json').write_text(json.dumps({'cycles':20, 'overlay_clicks':chips, 'underlay_clicks':clicks, 'window':window}, indent=2))
finally:
    try: command('quit')
    except OSError: pass
    # Underlay has its own bounded app timer; do not terminate unrelated processes.
    overlay.wait(timeout=100)
    underlay.wait(timeout=100)
    for file in logs.values(): file.close()
    control.unlink(missing_ok=True)
