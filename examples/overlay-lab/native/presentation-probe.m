// Diagnostic injection into overlay-lab only. No cross-process hooks are installed.
// Correlate Metal's actual presentedTime with the lab's CACurrentMediaTime show requests.
#import <AppKit/AppKit.h>
#import <QuartzCore/CAMetalLayer.h>
#import <Metal/Metal.h>
#import <objc/runtime.h>

static id<CAMetalDrawable> (*originalNext)(id, SEL);
static void (*originalOut)(id, SEL, id);
static void orderOut(NSWindow *window, SEL selector, id sender) {
    originalOut(window, selector, sender);
    if ([window isKindOfClass:NSPanel.class])
        fprintf(stderr, "probe hidden window=%ld visible=%d time=%.9f\n", (long)window.windowNumber, window.visible, CACurrentMediaTime());
}
static id<CAMetalDrawable> nextDrawable(id layer, SEL selector) {
    id<CAMetalDrawable> drawable = originalNext(layer, selector);
    if (!drawable) return drawable;
    NSInteger windowID = 0;
    for (NSWindow *window in NSApp.windows) {
        CALayer *root = window.contentView.layer;
        for (CALayer *p = layer; p; p = p.superlayer) {
            // Rune forwards source-window readbacks to the adopted content view. Its
            // real NSView.window identifies the panel rather than the hidden source.
            if (p == root) { windowID = window.contentView.window.windowNumber; break; }
        }
        if (windowID) break;
    }
    double acquired = CACurrentMediaTime();
    [drawable addPresentedHandler:^(id<MTLDrawable> d) {
        fprintf(stderr, "probe presented window=%ld acquired=%.9f presented=%.9f callback=%.9f\n",
                (long)windowID, acquired, d.presentedTime, CACurrentMediaTime());
    }];
    return drawable;
}
__attribute__((constructor)) static void install(void) {
    Method method = class_getInstanceMethod(CAMetalLayer.class, @selector(nextDrawable));
    originalNext = (void *)method_setImplementation(method, (IMP)nextDrawable);
    Method out = class_getInstanceMethod(NSWindow.class, @selector(orderOut:));
    originalOut = (void *)method_setImplementation(out, (IMP)orderOut);
}
