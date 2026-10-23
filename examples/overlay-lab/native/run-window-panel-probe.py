"""One bounded Pro GUI lease; serve explicit Cua/fixture actions under the GUI lock."""
import fcntl, json, os, pathlib, socket, subprocess, sys, time

assert socket.gethostname().startswith('Kits-Shiny-Computer')
root = pathlib.Path(sys.argv[1]).resolve()
root.mkdir(parents=True, exist_ok=True)
driver = str(pathlib.Path.home()/'.local/bin/cua-driver')
session = 'rune-window-panel'

def command(text):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(8)
        client.connect(str(root/'app.sock'))
        client.sendall((text+'\n').encode())
        return client.makefile().readline().strip()

with open('/tmp/pro-gui.lock', 'a') as lock, (root/'app.log').open('w') as log, (root/'actions.jsonl').open('w') as actions:
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    subprocess.run([driver, 'start_session', json.dumps({'session':session})], capture_output=True, check=True, timeout=15)
    with socket.socket(socket.AF_UNIX) as server:
        server.bind(str(root/'proof.sock'))
        server.listen()
        server.settimeout(600)
        app = subprocess.Popen([sys.argv[2]], env=dict(os.environ, WINDOW_PANEL_SOCKET=str(root/'app.sock')), stdout=log, stderr=subprocess.STDOUT)
        try:
            print(json.dumps({'ready':str(root/'proof.sock'),'pid':app.pid}), flush=True)
            while True:
                client, _ = server.accept()
                with client:
                    request = json.loads(client.makefile().readline())
                    try:
                        if 'command' in request:
                            result = command(request['command'])
                        elif 'tool' in request:
                            args = dict(request.get('args', {}), session=session)
                            run = subprocess.run([driver, request['tool'], json.dumps(args)], capture_output=True, text=True, timeout=45)
                            result = {'exit':run.returncode,'stdout':run.stdout,'stderr':run.stderr}
                        else:
                            result = 'finished'
                        actions.write(json.dumps({'time':time.time(),'request':request,'result':result})+'\n'); actions.flush()
                        client.sendall((json.dumps(result)+'\n').encode())
                        if request.get('finish'): break
                    except Exception as error:
                        client.sendall((json.dumps({'error':str(error)})+'\n').encode())
        finally:
            if app.poll() is None:
                try: command('quit'); app.wait(timeout=5)
                except Exception: app.terminate(); app.wait(timeout=5)
            subprocess.run([driver,'end_session',json.dumps({'session':session})], capture_output=True, timeout=15)
            for name in ['app.sock','proof.sock']: (root/name).unlink(missing_ok=True)
            print('Fixture exited; GUI lock released on exit.', flush=True)
