"""Compare the installed Metal pill; caller holds /tmp/pro-gui.lock on the authorized Mac.

Endpoint is native orderFrontRegardless entry during Scrawl's already-visible panel sync,
not the beginning of Scrawl's coordinator request. No redraw assistance is injected.
"""
import hashlib, json, math, os, pathlib, re, statistics, subprocess, sys

binary, root = map(pathlib.Path, sys.argv[1:3])
env = dict(os.environ, DYLD_INSERT_LIBRARIES=str(root/'presentation-probe.dylib'))
for key in ['OVERLAY_PROBE_REDRAW', 'OVERLAY_PROBE_NATIVE_OPTIONS']:
    env.pop(key, None)
args = [str(binary), 'lab', 'pill', 'tap-then-screen', '--repeat', '30',
        '--time-scale', '1', '--no-backdrop', '--trace', str(root/'motion.jsonl')]
(root/'load-before.txt').write_bytes(subprocess.check_output(['top','-l','1','-n','0']))
run = subprocess.run(args, env=env, capture_output=True, text=True, timeout=160)
(root/'presentation.log').write_text(run.stdout+run.stderr)
(root/'load-after.txt').write_bytes(subprocess.check_output(['top','-l','1','-n','0']))
if run.returncode: raise RuntimeError(f'Scrawl lab exited {run.returncode}; see presentation.log')
fronts, frames = [], []
for line in run.stderr.splitlines():
    front = re.match(r'probe front window=(\d+) requested=([\d.]+) was_visible=1 panel=1 level=30 view=(.*)',line)
    if front and 'ScrawlPillView' in front[3]: fronts.append((int(front[1]),float(front[2])))
    frame = re.match(r'probe presented window=(\d+) acquired=([\d.]+) presented=([\d.]+)',line)
    if frame: frames.append((int(frame[1]),float(frame[2]),float(frame[3])))
trials = []
for i, (window, requested) in enumerate(fronts):
    limit = min(requested+2, fronts[i+1][1]) if i+1 < len(fronts) else requested+2
    candidates = [presented for owner, acquired, presented in frames
                  if owner == window and requested <= acquired < limit and requested <= presented < limit]
    trials.append(dict(window=window, requested=requested,
        latency_ms=(min(candidates)-requested)*1000 if candidates else None))
samples = [t['latency_ms'] for t in trials if t['latency_ms'] is not None]
result = dict(binary=str(binary), sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
    profile='installed app; optimization profile not established',
    start='native orderFrontRegardless entry during already-visible pill sync',
    end='first positive Metal presentedTime acquired after that entry',
    diagnostic_redraw=False, trials=trials,
    min_ms=min(samples) if samples else None, median_ms=statistics.median(samples) if samples else None,
    p95_ms=sorted(samples)[math.ceil(len(samples)*.95)-1] if samples else None,
    max_ms=max(samples) if samples else None, under_16=sum(v<16 for v in samples),
    zero_callbacks=sum(presented==0 for _,_,presented in frames))
(root/'results.json').write_text(json.dumps(result,indent=2))
print(json.dumps({k:v for k,v in result.items() if k!='trials'},indent=2),flush=True)
print(f'trials={len(trials)} valid={len(samples)}',flush=True)
if len(trials)!=30 or len(samples)!=30: raise RuntimeError('Expected 30 native show markers and valid presentations')
