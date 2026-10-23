// Native input probe. Explicitly posts motion events; never substitutes cursor warps.
import ApplicationServices
import Foundation

let args = CommandLine.arguments
guard CGPreflightPostEventAccess() else {
    print("BLOCKED: input helper lacks event-posting permission; no permission requested")
    exit(77)
}
let source = CGEventSource(stateID: .hidSystemState)!
source.localEventsSuppressionInterval = 0
let from = CGPoint(x: Double(args[1])!, y: Double(args[2])!)
let to = CGPoint(x: Double(args[3])!, y: Double(args[4])!)
let steps = Int(args[5])!
for step in 0...steps {
    let t = Double(step) / Double(max(1, steps))
    let p = CGPoint(x: from.x + (to.x-from.x)*t, y: from.y + (to.y-from.y)*t)
    let event = CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left)!
    event.post(tap: .cghidEventTap)
    // Fixed input cadence, not a readiness poll. Exercise multiple native callbacks.
    Thread.sleep(forTimeInterval: 0.008)
}
if args.count > 6 && args[6] == "click" {
    for kind in [CGEventType.leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: source, mouseType: kind, mouseCursorPosition: to, mouseButton: .left)!.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.008)
    }
}
print("posted moves=\(steps+1) from=\(from) to=\(to) click=\(args.count > 6)")
