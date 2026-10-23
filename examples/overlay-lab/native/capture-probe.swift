import AppKit
import ScreenCaptureKit

@main struct CaptureProbe {
    static func main() async throws {
        let args = CommandLine.arguments
        let id = CGWindowID(args[1])!
        if args.count > 2 && args[2] == "--geometry" {
            let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []
            guard let match = windows.first(where: { ($0[kCGWindowNumber as String] as? UInt32) == id }),
                  let bounds = match[kCGWindowBounds as String] else { exit(1) }
            let data = try JSONSerialization.data(withJSONObject: bounds, options: [.sortedKeys])
            print(String(decoding: data, as: UTF8.self))
            return
        }
        if args.count == 2 || args[2] == "--hidden" {
            let started = ProcessInfo.processInfo.systemUptime
            var match: [String: Any]?
            // AppKit orderOut returns before WindowServer publishes its window list.
            // Bound this readiness check; it is outside the show/presentation interval.
            repeat {
                let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []
                match = windows.first { ($0[kCGWindowNumber as String] as? UInt32) == id }
                if args.count == 2 || (match?[kCGWindowIsOnscreen as String] as? Bool) != true { break }
                try await Task.sleep(for: .milliseconds(10))
            } while ProcessInfo.processInfo.systemUptime - started < 1
            print("window=\(id) exists=\(match != nil) onScreen=\(match?[kCGWindowIsOnscreen as String] ?? false)")
            return
        }
        guard CGPreflightScreenCaptureAccess() else {
            print("BLOCKED: ScreenCaptureKit probe lacks screen-recording permission; no permission requested")
            Foundation.exit(77)
        }
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let window = content.windows.first(where: { $0.windowID == id }),
              let display = content.displays.first(where: { $0.frame.intersects(window.frame) }) else {
            fatalError("window not found: \(id)")
        }
        print("window=\(id) frame=\(window.frame) onScreen=\(window.isOnScreen) pid=\(window.owningApplication?.processID ?? 0)")
        let config = SCStreamConfiguration()
        config.sourceRect = window.frame.offsetBy(dx: -display.frame.minX, dy: -display.frame.minY)
        config.width = Int(window.frame.width * 2)
        config.height = Int(window.frame.height * 2)
        config.showsCursor = false
        for exclude in [false, true] {
            let filter = SCContentFilter(display: display, excludingWindows: exclude ? [window] : [])
            let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
            let bitmap = NSBitmapImageRep(cgImage: image)
            let suffix = exclude ? "excluded" : "included"
            try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(args[2])-\(suffix).png"))
            print("capture=\(suffix) width=\(image.width) height=\(image.height)")
        }
    }
}
