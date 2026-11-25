# hello-floor

The least a Rune window holds in memory, and the floor under it.

- `hello-floor`: one Rune window with one line of text, and switches for what a window can do without.
- `hello-floor-bare`: the same window with no framework, stopped at each step of a Direct3D start. It shows what the API and the GPU's driver hold before Rune does anything (Windows).

The number is the **private working set**: what Task Manager shows as "Memory".

## Run it

```powershell
pwsh examples/hello-floor/measure.ps1                   # every step, on the adapter Rune picks
pwsh examples/hello-floor/measure.ps1 -Adapter nvidia   # on the adapter whose name holds "nvidia"
```

The script builds the two programs in release, opens one window at a time, waits 8 seconds, reads the process from outside and closes it. Run it for each adapter the machine has: the floor is the driver's, so it differs a lot between them.

To play with one step by hand:

| Switch | Values | What it does |
| --- | --- | --- |
| `HELLO_FLOOR_STEP` | `empty`, `text` (default), `effect` | A filled window; a line of text; the text under a frosted bar (a backdrop blur) |
| `HELLO_FLOOR_MEMORY_POLICY` | `memory-usage` (default), `performance`, `manual:<min>-<max>` in MiB | `App::gpu_memory_policy`: the size of the blocks the GPU allocator takes |
| `HELLO_FLOOR_WARM` | `none`, `backdrop`, `all` | `App::prewarm_effects`: which effect pipelines compile before their first use |
| `RUNE_CPU_FIRST` | `only`, `auto`, `off` | `only` keeps the window on Rune's CPU renderer: no GPU device until a frame needs one |
| `WGPU_ADAPTER_NAME` | part of an adapter's name | The GPU Rune draws on |
| `hello-floor-bare <api> <step>` | `win32`; `d3d12` or `d3d11` with `device`, `queue`, `swapchain`, `frame` | How far the bare program goes; `HELLO_FLOOR_ADAPTER` picks its adapter |

## The ladder

Private working set in MB. One laptop with two adapters: an Intel Iris Xe (integrated) and an NVIDIA GeForce RTX 3080 Ti. Windows 11, a 480 x 320 pt window, read 4 seconds (bare) or 10 seconds (Rune) after start, one run a row, 25 Nov 2026. The text row ran three times on each adapter: 22.2 to 22.3 on Intel, 69.6 to 70.1 on the RTX.

| Step | Intel Iris Xe | RTX 3080 Ti |
| --- | --: | --: |
| A window, filled by GDI | 1.0 | 1.0 |
| **Direct3D 12**: a device | 5.2 | 17.4 |
| + a command queue | 4.8 | 33.2 |
| + a two-buffer flip swap chain | 5.0 | 51.2 |
| + one cleared, presented frame | **5.4** | **60.2** |
| **Direct3D 11**: a device | 14.9 | 8.5 |
| + a swap chain | 19.1 | 26.0 |
| + one frame | **19.2** | **26.3** |
| **Rune** (Direct3D 12): an empty window | 23.3 | 69.1 |
| + a line of text | **22.2** | **69.6** |
| + a backdrop blur | 33.5 | 75.4 |
| The text window's first launch (Rune's own shader and pipeline caches cold) | 27.7 | 69.7 |

What Rune adds over the bare Direct3D 12 frame: 16.8 MB on Intel, 9.4 MB on the RTX.

### What each switch takes off the text window

| Switch | Intel Iris Xe | RTX 3080 Ti | |
| --- | --: | --: | --- |
| None | 22.2 | 69.6 | |
| `HELLO_FLOOR_MEMORY_POLICY=manual:4-64` | 22.1 | 69.8 | No change in this number. On the RTX the GPU's own dedicated memory went from 35.5 to 31.5 MB. |
| `HELLO_FLOOR_WARM=none` | 22.0 | 69.9 | No change: warming a pipeline costs no memory. |
| `RUNE_CPU_FIRST=only` | **9.5** | **9.7** | No Direct3D device at all. |

### Beside GPUI

GPUI's own hello on the same machine and day (Direct3D 11, read after 8 seconds by another harness): 37.5 MB on Intel, 31.9 MB on the RTX.

- On Intel, Rune's hello is under it: 22.2 against 37.5. A Direct3D 12 window costs 5.4 MB there and a Direct3D 11 one 19.2.
- On the RTX, Rune's hello is far over it: 69.6 against 31.9. Of those 37.7 MB, 33.9 are the API on that driver: a bare Direct3D 12 window holds 60.2 MB there (the device 17, its command queue 16, the swap chain 18, the first frame 9) and a bare Direct3D 11 one 26.3. Rune's own share on the RTX (9.4 MB) is the smallest part of its number.

### If Rune drew through Direct3D 11 on that adapter (estimated, not built)

| | Intel Iris Xe | RTX 3080 Ti |
| --- | --: | --: |
| Rune's text window today (Direct3D 12), measured | 22.2 | 69.6 |
| The same on Direct3D 11, **estimated** as the bare Direct3D 11 frame plus Rune's share today | about 36 | about 36 |
| Difference | about +14 | about -34 |

- The estimate assumes Rune's share is the same on both APIs. Nothing measures that: Rune has no Direct3D 11 renderer, and wgpu, which Rune draws through on Windows, has no Direct3D 11 backend.
- The cost is a second native renderer beside the Metal one, used only where it is the smaller: on this machine that is the RTX and not the Intel adapter.
- The measured way under both floors today is the CPU renderer row above: 9.5 and 9.7 MB, the same on either adapter, for as long as no frame needs the GPU.

## Not explained

- The empty window reads 1.1 MB over the text window on Intel (23.3 against 22.2) and 0.5 MB under it on the RTX. One run each.
- The first launch on Intel holds 5.5 MB more than the later ones. On the RTX it does not. Only Rune's own caches were cold in that row: Direct3D's shader cache for the program was not cleared between runs.
