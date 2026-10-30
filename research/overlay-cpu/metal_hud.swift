// A minimal hand-written Metal HUD for comparing per-frame CPU with Rune's overlay
// (crates/rune/examples/overlay_hud_cost.rs): a 360x80 borderless panel at the bottom of the
// screen whose CAMetalLayer is cleared to an animating colour from a CVDisplayLink callback,
// one command buffer per frame with the drawable presented on it.
//
//   swiftc -O metal_hud.swift -o metal_hud && ./metal_hud
//
// Env: HUD_SECONDS (default 12); HUD_EXTRA_CBS=n commits n empty command buffers a frame;
// HUD_SPLIT_PRESENT=1 presents from a command buffer of its own; HUD_DIRECT_PRESENT=1 commits,
// waits until the command buffer is scheduled and presents the drawable with no command buffer;
// HUD_HANDLERS=n adds a completion handler to n command buffers a frame (extras first);
// HUD_LABELS=1 labels each one; HUD_MAIN=1 draws on the main thread (the display link only
// wakes it). These mimic what wgpu does per frame (research/30-overlay-gpu-cost.md).
// Prints frames, fps and CPU ms per second for the main thread, the drawing thread and the rest.

import AppKit
import CoreVideo
import Metal
import QuartzCore

func env(_ name: String, _ fallback: Int) -> Int { Int(ProcessInfo.processInfo.environment[name] ?? "") ?? fallback }
let seconds = Double(env("HUD_SECONDS", 12))
let extraCBs = env("HUD_EXTRA_CBS", 0)
let splitPresent = env("HUD_SPLIT_PRESENT", 0) != 0
let directPresent = env("HUD_DIRECT_PRESENT", 0) != 0
let handlers = env("HUD_HANDLERS", 0)
let labels = env("HUD_LABELS", 0) != 0
let onMain = env("HUD_MAIN", 0) != 0

func threadCPU() -> Double {
    var t = timespec()
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &t)
    return Double(t.tv_sec) * 1e3 + Double(t.tv_nsec) / 1e6
}
func processCPU() -> Double {
    var u = rusage()
    getrusage(RUSAGE_SELF, &u)
    func ms(_ t: timeval) -> Double { Double(t.tv_sec) * 1e3 + Double(t.tv_usec) / 1e3 }
    return ms(u.ru_utime) + ms(u.ru_stime)
}

func processCounts() -> [UInt64] {
    var info = rusage_info_v4()
    let ok = withUnsafeMutablePointer(to: &info) { p in
        p.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(getpid(), RUSAGE_INFO_V4, $0) }
    }
    return ok == 0 ? [info.ri_instructions, info.ri_cycles, info.ri_interrupt_wkups] : [0, 0, 0]
}

final class Hud {
    let device = MTLCreateSystemDefaultDevice()!
    lazy var queue = device.makeCommandQueue()!
    let layer = CAMetalLayer()
    let start = Date()
    var frames = 0
    var drawCPU = 0.0
    var mainCPU0 = 0.0, process0 = 0.0, draw0 = 0.0, frames0 = 0, wall0 = Date()
    var warm = false
    var counts0: [UInt64] = [0, 0, 0]

    var handled = 0

    func frame() {
        handled = 0
        let entered = threadCPU()
        frames += 1
        let elapsed = Date().timeIntervalSince(start)
        if !warm && elapsed >= 2 {
            warm = true
            process0 = processCPU(); counts0 = processCounts(); draw0 = drawCPU; frames0 = frames; wall0 = Date()
            DispatchQueue.main.async { self.mainCPU0 = threadCPU() }
        }
        if warm && elapsed >= seconds {
            let wall = Date().timeIntervalSince(wall0)
            let n = Double(frames - frames0)
            let draw = drawCPU - draw0
            let process = processCPU() - process0
            let counts = processCounts()
            let per = { (i: Int) in Double(counts[i] - self.counts0[i]) / n }
            DispatchQueue.main.async {
                let main = threadCPU() - self.mainCPU0
                let drawing = onMain ? 0 : draw
                print(String(format: "frames=%.0f fps=%.1f main_ms_per_s=%.1f draw_thread_ms_per_s=%.1f other_ms_per_s=%.1f process_cpu_pct=%.1f draw_us_per_frame=%.1f",
                             n, n / wall, main / wall, drawing / wall, (process - main - drawing) / wall, process / wall / 10, draw * 1e3 / n))
                print(String(format: "process_per_frame: kinstructions=%.1f kcycles=%.1f wakeups=%.2f", per(0) / 1e3, per(1) / 1e3, per(2)))
                exit(0)
            }
        }
        autoreleasepool {
            guard let drawable = layer.nextDrawable() else { return }
            let phase = sin(elapsed * 2) * 0.5 + 0.5
            for _ in 0..<extraCBs { commit(queue.makeCommandBufferWithUnretainedReferences()!, "extra") }
            let cb = queue.makeCommandBufferWithUnretainedReferences()!
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = drawable.texture
            pass.colorAttachments[0].loadAction = .clear
            pass.colorAttachments[0].storeAction = .store
            pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: phase * 0.4, alpha: 0.4)
            cb.makeRenderCommandEncoder(descriptor: pass)!.endEncoding()
            if directPresent {
                commit(cb, "frame")
                cb.waitUntilScheduled()
                drawable.present()
            } else if splitPresent {
                commit(cb, "frame")
                let present = queue.makeCommandBuffer()!
                present.present(drawable)
                commit(present, "present")
            } else {
                cb.present(drawable)
                commit(cb, "frame")
            }
        }
        drawCPU += threadCPU() - entered
    }

    func commit(_ cb: MTLCommandBuffer, _ label: String) {
        if labels { cb.label = label }
        if handled < handlers { handled += 1; cb.addCompletedHandler { _ in } }
        cb.commit()
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let hud = Hud()
let screen = NSScreen.main!.frame
let size = NSSize(width: 360, height: 80)
let panel = NSPanel(contentRect: NSRect(x: screen.midX - size.width / 2, y: screen.minY + 60, width: size.width, height: size.height),
                    styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
panel.isOpaque = false
panel.backgroundColor = .clear
panel.level = .statusBar
panel.ignoresMouseEvents = true
let view = NSView(frame: NSRect(origin: .zero, size: size))
view.wantsLayer = true
hud.layer.device = hud.device
hud.layer.pixelFormat = .bgra8Unorm
hud.layer.isOpaque = false
hud.layer.framebufferOnly = true
hud.layer.contentsScale = panel.backingScaleFactor
hud.layer.drawableSize = CGSize(width: size.width * panel.backingScaleFactor, height: size.height * panel.backingScaleFactor)
view.layer = hud.layer
panel.contentView = view
panel.orderFrontRegardless()

var link: CVDisplayLink?
CVDisplayLinkCreateWithActiveCGDisplays(&link)
CVDisplayLinkSetOutputHandler(link!) { _, _, _, _, _ in
    if onMain { DispatchQueue.main.async { hud.frame() } } else { hud.frame() }
    return kCVReturnSuccess
}
CVDisplayLinkStart(link!)
DispatchQueue.main.asyncAfter(deadline: .now() + seconds + 20) { print("metal_hud: no frames"); exit(1) }
app.run()
