"""Assert native alpha-zero drawable acquisition before an overlay's creation callback."""
import re


def assert_prewarmed(lines, window):
    acquired = False
    for line in lines:
        match = re.match(r'probe acquired window=(\d+) alpha=([\d.]+) time=', line)
        if match and int(match[1]) == window:
            acquired |= float(match[2]) == 0.0
        if ' created OverlayCreated ' in line and f'window_number: Some({window})' in line:
            if not acquired:
                raise AssertionError(f'window {window}: no alpha-zero drawable acquired before on_created; check prewarm/occlusion')
            return
    raise AssertionError(f'window {window}: missing on_created marker')


if __name__ == '__main__':
    import pathlib
    import sys
    lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
    windows = [int(match[1]) for line in lines if ' created OverlayCreated ' in line
               and (match := re.search(r'window_number: Some\((\d+)\)', line))]
    if not windows:
        raise AssertionError('no created overlays in trace')
    for window in windows:
        assert_prewarmed(lines, window)
    print(f'Alpha-zero prewarm verified before on_created for {len(windows)} overlays: {windows}')
