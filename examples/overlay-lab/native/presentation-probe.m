// Diagnostic injection into overlay-lab only. No cross-process hooks are installed.
// Correlate Metal's actual presentedTime with the lab's CACurrentMediaTime show requests.
#import <AppKit/AppKit.h>
#import <QuartzCore/CAMetalLayer.h>
#import <Metal/Metal.h>
#import <objc/runtime.h>

static id<CAMetalDrawable> (*originalNext)(id, SEL);
static void (*originalOut)(id, SEL, id);
static void (*originalAlpha)(id, SEL, CGFloat);
static void (*originalFront)(id, SEL);
static void orderFront(NSWindow *window, SEL selector) {
    double requested = CACurrentMediaTime();
    BOOL wasVisible = window.visible;
    originalFront(window, selector);
    fprintf(stderr, "probe front window=%ld requested=%.9f was_visible=%d panel=%d level=%ld view=%s\n",
        (long)window.windowNumber, requested, wasVisible, [window isKindOfClass:NSPanel.class],
        (long)window.level, class_getName(window.contentView.class));
}
static void setAlpha(NSWindow *window, SEL selector, CGFloat alpha) {
    originalAlpha(window, selector, alpha);
    if (![window isKindOfClass:NSPanel.class]) return;
    fprintf(stderr, "probe alpha window=%ld alpha=%.1f time=%.9f\n", (long)window.windowNumber, alpha, CACurrentMediaTime());
    // Opt-in measurement assistance, not Rune behavior. Alpha-only reveal reuses a
    // previously submitted drawable; Metal cannot timestamp its compositor reveal.
    if (alpha == 1 && getenv("OVERLAY_PROBE_REDRAW")) {
        double deadline = CACurrentMediaTime() + 2;
        objc_setAssociatedObject(window, @selector(setAlphaValue:), @(deadline), OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        window.contentView.needsDisplay = YES;
    }
}
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
    __weak NSWindow *owner;
    for (NSWindow *window in NSApp.windows)
        if (window.windowNumber == windowID) { owner = window; break; }
    fprintf(stderr, "probe acquired window=%ld alpha=%.1f time=%.9f\n", (long)windowID, owner.alphaValue, acquired);
    if (getenv("OVERLAY_PROBE_NATIVE_OPTIONS"))
        fprintf(stderr, "probe native window=%ld shadow=%d level=%ld collection=%lu sharing=%lu\n",
            (long)owner.windowNumber, owner.hasShadow, (long)owner.level,
            (unsigned long)owner.collectionBehavior, (unsigned long)owner.sharingType);
    [drawable addPresentedHandler:^(id<MTLDrawable> d) {
        fprintf(stderr, "probe presented window=%ld acquired=%.9f presented=%.9f callback=%.9f\n",
                (long)windowID, acquired, d.presentedTime, CACurrentMediaTime());
        if (d.presentedTime == 0 && getenv("OVERLAY_PROBE_REDRAW")) dispatch_async(dispatch_get_main_queue(), ^{
            NSWindow *window = owner;
            NSNumber *deadline = objc_getAssociatedObject(window, @selector(setAlphaValue:));
            if (window.alphaValue == 1 && deadline.doubleValue > CACurrentMediaTime()) window.contentView.needsDisplay = YES;
        });
    }];
    return drawable;
}
__attribute__((constructor)) static void install(void) {
    Method method = class_getInstanceMethod(CAMetalLayer.class, @selector(nextDrawable));
    originalNext = (void *)method_setImplementation(method, (IMP)nextDrawable);
    Method out = class_getInstanceMethod(NSWindow.class, @selector(orderOut:));
    originalOut = (void *)method_setImplementation(out, (IMP)orderOut);
    Method alpha = class_getInstanceMethod(NSWindow.class, @selector(setAlphaValue:));
    originalAlpha = (void *)method_setImplementation(alpha, (IMP)setAlpha);
    Method front = class_getInstanceMethod(NSWindow.class, @selector(orderFrontRegardless));
    originalFront = (void *)method_setImplementation(front, (IMP)orderFront);
}
