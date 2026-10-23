"""Authorized Pro desktop proof via CuaDriver; holds the GUI lock for the whole run.

prepare starts a bounded worker and returns after its grounding screenshot. Read that
image, then send {"run":true,"scale":2} to cua-control.sock to run the proof. The
worker streams its result on that socket and exits, closing its own apps/session.
"""
import ast, fcntl, json, os, pathlib, re, selectors, socket, subprocess, sys, threading, time

root = pathlib.Path(sys.argv[1]).resolve()
driver = str(pathlib.Path.home()/'.local/bin/cua-driver')
session = 'rune-overlay-boundary'
target = {'kind': 'desktop', 'display_id': 'primary'}

if len(sys.argv) > 2 and sys.argv[2] == 'prepare':
    with (root/'worker.log').open('w') as log:
        worker = subprocess.Popen([sys.executable, '-u', __file__, str(root)],
                                  stdout=subprocess.PIPE, stderr=log, start_new_session=True)
        ready = selectors.DefaultSelector()
        ready.register(worker.stdout, selectors.EVENT_READ)
        if not ready.select(90): raise TimeoutError('worker did not prepare in 90 seconds')
        line = worker.stdout.readline().decode()
        if not line: raise RuntimeError('worker exited; inspect worker.log')
        print(line, end='')
    sys.exit(0)

lock = open('/tmp/pro-gui.lock', 'a')
fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
if subprocess.run(['pgrep', '-f', '[r]une-smoke/ci/Hex.app'], capture_output=True).returncode == 0:
    raise RuntimeError('competing Hex smoke app')
condition = threading.Condition()
state = {'clicks': 0, 'chips': 0, 'window': None, 'trace': None, 'seq': 0, 'regions': []}
actions = (root/'actions.jsonl').open('w')
steps = (root/'steps.jsonl').open('w')
apps = []

def record(file, value):
    file.write(json.dumps(value)+'\n'); file.flush()

def cua(tool, **args):
    payload = dict(args, session=session)
    run = subprocess.run([driver, tool, json.dumps(payload)], capture_output=True, text=True, timeout=35)
    record(actions, {'tool': tool, 'args': payload, 'stdout': run.stdout, 'stderr': run.stderr, 'exit': run.returncode})
    if run.returncode: raise RuntimeError(f'Cua {tool}: {run.stdout} {run.stderr}')
    result = json.loads(run.stdout)
    if result.get('isError') or result.get('error'): raise RuntimeError(f'Cua {tool}: {result}')
    return result

def read_logs(name, app):
    with (root/f'input-{name}.log').open('w') as log:
        for line in app.stdout:
            log.write(line); log.flush()
            with condition:
                if name == 'underlay' and 'underlay click ' in line:
                    state['clicks'] = int(line.rsplit(' ', 1)[1])
                if name == 'overlay' and 'overlay-lab chip iPhone' in line: state['chips'] += 1
                if name == 'overlay' and 'pill created' in line:
                    state['window'] = int(re.search(r'window_number: Some\((\d+)\)', line)[1])
                if name == 'overlay' and line.startswith('overlay-input '):
                    window = int(re.search(r'window=(\d+)', line)[1])
                    if window == state['window']:
                        state['seq'] += 1
                        state['regions'] = ast.literal_eval(line.split('regions=', 1)[1])
                        state['trace'] = line.strip()
                condition.notify_all()

def wait(test, seconds=8):
    with condition:
        if not condition.wait_for(lambda: test(state), timeout=seconds):
            raise TimeoutError(f'predicate timed out: {state}')
        return dict(state)

def snapshot(label, fresh=True, ignore=None):
    with condition: seq = state['seq']
    # Region publication logs precede observe() in the same frame. Wait for a
    # subsequent poll readback, not that intentionally pre-update diagnostic row.
    value = wait(lambda s: s['seq'] > seq and 'source=poll ' in (s['trace'] or '')
                 and (ignore is None or f'ignore={str(ignore).lower()}' in s['trace'])) if fresh else dict(state)
    record(steps, {'step': label, **value})
    return value

def command(value):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(8); client.connect(str(root/'input.sock'))
        client.sendall((value+'\n').encode())
        result = client.recv(4096).decode()
        record(actions, {'command': value, 'result': result})

def launch(name, **extra):
    env = dict(os.environ, OVERLAY_LAB_SECONDS='600', RUNE_OVERLAY_TRACE_INPUT='1',
               OVERLAY_LAB_INCLUDE_IN_CAPTURE='1', **extra)
    app = subprocess.Popen([str(root/'overlay-lab')], env=env, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, text=True, bufsize=1)
    apps.append(app)
    threading.Thread(target=read_logs, args=(name, app), daemon=True).start()
    return app

def move(point):
    cua('move_cursor', target=target, x=point[0]*scale, y=point[1]*scale)

def click(point):
    cua('click', target=target, x=point[0]*scale, y=point[1]*scale)

def center(region):
    return (origin[0]+(region[0]+region[2])/2, origin[1]+(region[1]+region[3])/2)

def drag_while(start, end, action):
    result = []
    def run():
        try:
            result.append(cua('drag', target=target, from_x=start[0]*scale, from_y=start[1]*scale,
                              to_x=end[0]*scale, to_y=end[1]*scale, duration_ms=2500, steps=100))
        except Exception as error: result.append(error)
    thread = threading.Thread(target=run)
    thread.start()
    try:
        wait(lambda s: s['trace'] and 'captured=true' in s['trace'] and 'buttons=1 ' in s['trace'])
        snapshot('held-before-'+action)
        command(action)
        if action == 'none':
            current = snapshot('held-after-none')
            assert 'input=Regions ' in current['trace'] and 'captured=true' in current['trace']
    finally:
        thread.join(timeout=35)
    if thread.is_alive(): raise TimeoutError('Cua drag did not return')
    if not result or isinstance(result[0], Exception): raise RuntimeError(str(result))

connection = None
try:
    cua('start_session')
    launch('underlay', OVERLAY_LAB_UNDERLAY='1')
    overlay = launch('overlay', OVERLAY_LAB_CONTROL=str(root/'input.sock'))
    wait(lambda s: s['window'] is not None)
    command('show')
    wait(lambda s: len(s['regions']) >= 4)
    geometry = json.loads(subprocess.check_output([str(root/'capture-probe'), str(state['window']), '--geometry'], text=True))
    origin = (geometry['X'], geometry['Y'])
    desktop = cua('get_desktop_state', screenshot_out_file=str(root/'grounding.png'))
    (root/'grounding.json').write_text(json.dumps(desktop, indent=2))
    server = socket.socket(socket.AF_UNIX)
    server.bind(str(root/'cua-control.sock')); server.listen(1); server.settimeout(120)
    print(json.dumps({'ready': True, 'worker': os.getpid(), 'window': state['window'], 'geometry': geometry,
                      'screenshot': str(root/'grounding.png')}), flush=True)
    connection, _ = server.accept(); connection.settimeout(400)
    request = json.loads(connection.recv(4096))
    if not request.get('run'): raise RuntimeError('run not authorized')
    scale = request['scale']
    cycles = request.get('cycles', 20)
    hide_case = request.get('hide_case', True)
    outside = (origin[0]+20, origin[1]+80)
    move(outside)
    initial = snapshot('initial-outside', ignore=True)
    assert 'ignore=true' in initial['trace']
    for trial in range(cycles):
        inside = center(state['regions'][3])
        move(inside)
        entered = snapshot(f'{trial+1}:enter', ignore=False)
        assert 'ignore=false' in entered['trace'], entered
        click(inside)
        wait(lambda s: s['chips'] == trial+1 or s['clicks'] > trial)
        pressed = snapshot(f'{trial+1}:inside-click')
        assert pressed['chips'] == trial+1 and pressed['clicks'] == trial, pressed
        move(outside)
        left = snapshot(f'{trial+1}:leave', ignore=True)
        assert 'ignore=true' in left['trace'], left
        click(outside)
        wait(lambda s: s['clicks'] == trial+1)
        passed = snapshot(f'{trial+1}:outside-click')
        assert passed['chips'] == trial+1 and passed['clicks'] == trial+1, passed
    (root/'boundary-results.json').write_text(json.dumps({'cycles': cycles, 'overlay_clicks': state['chips'],
        'underlay_clicks': state['clicks'], 'window': state['window']}, indent=2))
    # Cancel an owned press by hiding mid-drag, then prove both routes recover.
    if hide_case:
        inside = center(state['regions'][3])
        move(inside)
        drag_while(inside, outside, 'hide')
        command('show')
        move(outside); click(outside)
        wait(lambda s: s['clicks'] == cycles+1)
        snapshot('hide-mid-press-outside-recovered')
        inside = center(state['regions'][3])
        move(inside); click(inside)
        wait(lambda s: s['chips'] == cycles+1)
        snapshot('hide-mid-press-inside-recovered')
    count = cycles + int(hide_case)
    # No Runtime press/drag handler: native capture alone must defer None.
    command('hit-only')
    wait(lambda s: len(s['regions']) == 1)
    hit = center(state['regions'][0])
    move(hit)
    click(hit)
    bare = snapshot('hit-only-click')
    assert bare['clicks'] == count and bare['chips'] == count, bare
    drag_while(hit, outside, 'none')
    # None stops polling. The final release row can precede applying the deferred
    # mode, so prove None by actual cross-process delivery instead of another tick.
    wait(lambda s: 'captured=false' in (s['trace'] or ''))
    snapshot('hit-only-none-after-up', fresh=False)
    move(hit); click(hit)
    wait(lambda s: s['clicks'] == count+1)
    record(steps, {'step': 'hit-only-through-after-none', **state})
    command('regions')
    move(outside)
    snapshot('regions-restored')
    cua('get_desktop_state', screenshot_out_file=str(root/'final.png'))
    result = {'cycles': cycles, 'overlay_clicks': cycles, 'underlay_clicks': cycles,
              'hide_mid_press': 'passed' if hide_case else 'not run', 'handlerless_hit_none_mid_press': 'passed',
              'final_overlay_clicks': state['chips'], 'final_underlay_clicks': state['clicks'],
              'window': state['window'], 'input_driver': 'CuaDriver desktop global input',
              'lost_up_limit': 'hide-mid-press cancellation; 100ms recovery remains unit-covered'}
    (root/'results.json').write_text(json.dumps(result, indent=2))
    connection.sendall((json.dumps(result)+'\n').encode())
except Exception as error:
    (root/'failure.txt').write_text(repr(error))
    try: cua('get_desktop_state', screenshot_out_file=str(root/'failure.png'))
    except Exception: pass
    if connection:
        connection.sendall((json.dumps({'error': repr(error)})+'\n').encode())
    raise
finally:
    try: command('quit')
    except OSError: pass
    for app in apps:
        if app.poll() is None:
            # These are the two disposable probe processes created above, not user apps.
            app.terminate()
        app.wait(timeout=15)
    try: cua('end_session')
    except Exception as error: record(actions, {'cleanup_error': repr(error)})
    for name in ['input.sock', 'cua-control.sock']: (root/name).unlink(missing_ok=True)
    actions.close(); steps.close()
    lock.close()
    (root/'finished').write_text('apps exited; Cua session ended; GUI lock released\n')
    if connection: connection.close()
