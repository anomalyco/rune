// Rune's floor under an app's launch: `examples/launch-floor` from the process's start to its first
// frame, on the bare window (`bare`) and on one shaped like a desktop app's first screen (`shell`).
//
//   strobe bench perf/launch.bench.ts --head . --check                          does it launch? each stage once
//   strobe bench perf/launch.bench.ts --base origin/dev --head .                did this branch move the floor?
//   strobe bench perf/launch.bench.ts --base exe:<old> --head exe:<new>         two prebuilt floor apps
//   strobe record --bench perf/launch.bench.ts#warm --head . --target shell     stacks: where the launch's time goes
//   strobe query <rec> hotspots --thread main --sort total                      ... by function, file:line
//
// The app prints each of Rune's launch marks with the mark's own time (`launch-mark <name> <µs since
// the process started> at=<epoch ms>`), 300 ms after its first frame, so printing is never inside
// what is timed. `LAUNCH_FLOOR_TRACE=<file>` in the environment keeps every launch's marks in µs.
import { bench, native } from "@hona/strobe"

const mark = (name: string) => ({ match: new RegExp(`launch-mark ${name.replace(/\./g, "\\.")} \\d+ at=(?<at>\\d{13})`), in: "output" as const })

// Rune's stages in the order a launch reaches them.
const milestones = {
  main: mark("main.entered"),
  loop_built: mark("loop.built"),
  window_native: mark("window.native"),
  window_created: mark("window.created"),
  first_layout: mark("frame.first.layout"),
  first_painted: mark("frame.first.painted"),
  first_frame: mark("frame.first.presented"),
  input_ready: mark("input.ready"),
}
const stages = ["launch.main", "launch.loop_built", "launch.window_native", "launch.window_created", "launch.first_layout", "launch.first_painted", "launch.input_ready"]
const outside = ["cpu.work", "memory.app.private"]
// Windows: the window's first frame is the CPU's; these are the GPU being made behind it and taking the window over.
const handover = { ...milestones, gpu_device: mark("gpu.device"), gpu_warmed: mark("gpu.window.warmed"), gpu_handed: mark("gpu.window.handed") }
const floors = ["bare", "shell"]

export default bench({
  targets: {
    bare: native("floor", { name: "Rune floor: one text, one rectangle", output: true }),
    shell: native("floor-shell", { name: "Rune floor: a desktop app's first screen", output: true }),
    // The same screen, reporting once the GPU has taken the window over (Windows).
    handover: native("floor-handover", { name: "Rune floor: to the GPU's first frame", output: true }),
    "handover-serial": native("floor-handover-serial", { name: "Rune floor: to the GPU's first frame, its pipelines made one after the other", output: true }),
  },
  cases: {
    // A later launch: the exe in the file cache, the launch's home kept, the GPU's shader cache warm.
    warm: {
      primary: "launch.ready",
      hero: true,
      targets: floors,
      metrics: [...stages, ...outside],
      launch: { until: { milestone: "first_frame" }, milestones, profile: "warm" },
    },
    // A launch on a new home with the GPU driver's shader cache emptied: what the first frame takes
    // when nothing of an earlier launch is left but the exe's pages.
    cold: {
      primary: "launch.ready",
      gpuCache: "cold",
      targets: floors,
      metrics: [...stages, ...outside],
      launch: { until: { milestone: "first_frame" }, milestones, profile: "fresh" },
    },
    // To the loop waiting for input after the first frame.
    input: {
      primary: "launch.ready",
      targets: floors,
      metrics: ["launch.first_frame", "cpu.work"],
      launch: { until: { milestone: "input_ready" }, milestones, profile: "warm" },
    },
    // Windows, a machine that has never run the app: no shader cache, so the driver compiles each
    // pipeline as it is created. To the frame the GPU takes the window over with (until then the
    // CPU draws every frame).
    //   strobe bench perf/launch.bench.ts --base handover-serial --head handover --filter handover
    handover: {
      primary: "launch.ready",
      gpuCache: "cold",
      targets: ["handover", "handover-serial"],
      metrics: ["launch.first_frame", "launch.gpu_device", "launch.gpu_warmed", "cpu.work"],
      launch: { until: { milestone: "gpu_handed" }, milestones: handover, profile: "fresh" },
    },
  },
})
