#!/usr/bin/env python3
"""Repeatable offscreen workloads; build and run on a headless GPU worker.

Frame-CPU mode:
  cargo build --profile perf -p settings --bins --features frame-profile
  python3 scripts/bench-ui.py --frame-cpu --out target/frame-cpu --runs 9

Scroll frame-CPU mode (windowless; input through draw lists, no GPU submission):
  cargo build --profile perf -p rune-kit --example scroll_bench --features markdown,frame-profile
  python3 scripts/bench-ui.py --frame-cpu --scroll --out target/scroll --runs 9
Save scroll_bench under saved/target/perf/examples/ to compare scroll workloads.

Save the baseline binaries in saved/target/perf,
then build the candidate and add --compare saved/target/perf. Use separate Cargo
target directories for separate source roots. Comparison verifies binary hashes
differ, discards one warmup per side, and alternates paired runs; report medians
and MAD, and keep timing changes only above the observed noise.

Commit includes view/layout/shaping, and Layout includes shaping: phases overlap;
do not sum them. CPU clocks measure the calling thread; elapsed encode/submit may
include queue backpressure. The runner forces idle frames, so idle is not native
wake CPU; startup measures first completed pixels, not display presentation.
"""
import argparse
import hashlib
import json
import pathlib
import re
import statistics
import subprocess
import os
import selectors
import time

from perf_binary import perf_binary

ROOT = pathlib.Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
p.add_argument('--out', required=True, type=pathlib.Path)
p.add_argument('--runs', type=int, default=7)
p.add_argument('--only', default='')
p.add_argument('--frame-cpu', action='store_true', help='include dense fixtures and phase/allocation metrics (build --features frame-profile)')
p.add_argument('--scroll', action='store_true', help='windowless scroll workloads; build rune-kit example scroll_bench with markdown,frame-profile')
p.add_argument('--compare', type=pathlib.Path, help='alternate with saved baseline binaries in a target/perf directory')
a = p.parse_args()
if a.runs < 1:
    p.error('--runs must be positive')
a.out.mkdir(parents=True, exist_ok=True)
workloads = {
    'settings': ('settings', ['--script', 'examples/settings/scripts/bench.json', '--frames', '360']),
}
if a.frame_cpu:
    workloads['startup'] = ('settings', ['--first-pixels', '--frames', '1'])
if a.scroll:
    workloads = {f'scroll-{name}': ('examples/scroll_bench', ['--workload', name, '--frames', '480'])
                 for name in ('list', 'markdown', 'sticky', 'growth', 'prose')}
rx = re.compile(r'gpu init ([\d.]+) ms, first frame ([\d.]+) ms, frame cpu mean ([\d.]+) ms p50 ([\d.]+) p95 ([\d.]+) max ([\d.]+), (\d+) instances')
names = ['gpu_ms', 'first_ms', 'mean_ms', 'p50_ms', 'p95_ms', 'max_ms', 'instances']
result = {}
targets = {'current': pathlib.Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')) / 'perf'}
if a.compare:
    targets['before'] = a.compare.resolve()
for name, (binary, args) in workloads.items():
    if a.only and name not in a.only.split(','):
        continue
    binaries = {stage: perf_binary(target / binary) for stage, target in targets.items()}
    if a.compare:
        hashes = []
        for path in binaries.values():
            digest = hashlib.sha256()
            with path.open('rb') as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b''):
                    digest.update(chunk)
            hashes.append(digest.digest())
        if hashes[0] == hashes[1]:
            p.error(f'{name}: current and before binaries have identical SHA-256 hashes; rebuild separate candidates')
    samples = {stage: [] for stage in targets}
    for i in range(a.runs + 1):
        for stage in list(targets)[::(-1 if i % 2 else 1)]:
            cmd = [str(binaries[stage]), *args, '--fps', '120']
            process_ms = None
            log_path = a.out / f'{name}-{stage}-{i}.log'
            if name == 'startup':
                with log_path.open('w+') as log:
                    start = time.perf_counter()
                    with subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=log, text=True) as proc:
                        try:
                            with selectors.DefaultSelector() as selector:
                                selector.register(proc.stdout, selectors.EVENT_READ)
                                if not selector.select(timeout=30):
                                    raise TimeoutError('first pixels not ready after 30s')
                                first = proc.stdout.readline()
                            process_ms = (time.perf_counter() - start) * 1000
                            if not first.startswith('rune-first-pixels '):
                                raise RuntimeError(f'unexpected first-pixels marker: {first}')
                            stdout, _ = proc.communicate(timeout=30)
                            log.seek(0)
                            r = subprocess.CompletedProcess(cmd, proc.returncode, first + stdout, log.read())
                            r.check_returncode()
                        finally:
                            if proc.poll() is None:
                                proc.kill()
                                proc.wait()
            else:
                r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, check=True, timeout=180)
                log_path.write_text(r.stdout + r.stderr)
            if a.scroll:
                profile = re.search(r'rune-scroll-profile (.+)', r.stdout)
                if not profile:
                    raise RuntimeError(r.stdout + r.stderr)
                if i:
                    samples[stage].append(json.loads(profile[1]))
                continue
            m = rx.search(r.stderr)
            if not m:
                raise RuntimeError(r.stderr)
            row = dict(zip(names, map(float, m.groups())))
            phase = re.search(r'rune-frame-profile (.+)', r.stderr)
            if a.frame_cpu and not phase:
                raise RuntimeError('rebuild with --features frame-profile')
            if phase:
                row.update({k: v for k, v in json.loads(phase[1]).items() if v is not None})
            pixels = re.search(r'rune-first-pixels (.+)', r.stdout)
            if pixels:
                row.update(json.loads(pixels[1]))
            if process_ms is not None:
                row['process_ms'] = process_ms
            if i:
                samples[stage].append(row)
    for stage, runs in samples.items():
        med = {k: statistics.median(r[k] for r in runs) for k in runs[0]}
        mad = {k: statistics.median(abs(r[k] - med[k]) for r in runs) for k in med}
        label = f'{name}.{stage}' if a.compare else name
        result[label] = {'median': med, 'mad': mad, 'runs': runs}
        print(label, json.dumps(med), flush=True)
        for key, value in med.items():
            print(f'METRIC {label}.{key}={value:.6f}', flush=True)
            print(f'METRIC {label}.{key}.mad={mad[key]:.6f}', flush=True)
    if a.compare:
        metric = 'cpu_ms_p50' if a.scroll else ('thread_cpu_ms' if 'thread_cpu_ms' in samples['current'][0] and 'thread_cpu_ms' in samples['before'][0] else 'mean_ms')
        ratios = [cur[metric] / old[metric] for cur, old in zip(samples['current'], samples['before'])]
        ratio = statistics.median(ratios)
        spread = statistics.median(abs(r - ratio) for r in ratios)
        result[f'{name}.paired'] = {'metric': metric, 'ratio': ratio, 'mad': spread}
        print(f'{name}: paired ratio uses {metric}', flush=True)
        print(f'METRIC {name}.paired_ratio={ratio:.6f}', flush=True)
        print(f'METRIC {name}.paired_ratio.mad={spread:.6f}', flush=True)
    (a.out / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
