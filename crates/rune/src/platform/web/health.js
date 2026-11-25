// A WebGPU device's errors and loss, read from the JS device itself. wgpu's own handlers turn a
// GPUInternalError into a panic (an `unreachable` trap that ends the app), so Rune never asks
// wgpu for them on the web.

// Whether `navigator.gpu` is exposed on this page (false on insecure origins or when WebGPU is
// disabled by flag, even when `canvas.getContext('webgpu')` still returns a context).
export function hasNavigatorGpu() {
    return typeof globalThis.navigator?.gpu === 'object' && globalThis.navigator.gpu !== null;
}

// The adapter a device runs on, as far as the browser says: vendor, architecture, device and
// description (each may be empty), whether it is a fallback adapter, and its texture limit.
export function gpuSummary(device) {
    const info = device.adapterInfo ?? {};
    const parts = [info.vendor, info.architecture, info.device, info.description].filter(Boolean);
    if (info.isFallbackAdapter) parts.push('fallback adapter');
    parts.push(`max texture ${device.limits.maxTextureDimension2D}`);
    return parts.join(', ');
}

// Calls `onError(kind, message)` for each uncaptured error and keeps it out of the browser's own
// log (`onError` logs what's worth logging), and `onLost(reason, message)` once if the device is
// lost for any reason but being destroyed.
export function watchGpu(device, onError, onLost, terminal) {
    device.addEventListener('uncapturederror', (event) => {
        event.preventDefault();
        onError(event.error?.constructor?.name ?? 'GPUError', event.error?.message ?? '');
    });
    device.lost.then((info) => {
        if (terminal || info.reason !== 'destroyed') onLost(info.reason ?? 'unknown', info.message ?? '');
    });
}

// Clears any single-slot JS `device.onuncapturederror` handler installed by an embedded renderer
// while leaving Rune's `addEventListener('uncapturederror', ...)` and `device.lost.then(...)`
// listeners intact without registering duplicates.
export function restoreGpu(device) {
    if (device && 'onuncapturederror' in device) {
        device.onuncapturederror = null;
    }
}

export function pushScopes(device) {
    device.pushErrorScope('validation');
    device.pushErrorScope('out-of-memory');
    device.pushErrorScope('internal');
}

// Pops `pushScopes`'s scopes now, and resolves to the first error they caught as `kind:
// message`, or null.
export function popScopes(device) {
    const caught = [device.popErrorScope(), device.popErrorScope(), device.popErrorScope()];
    return Promise.all(caught).then((errors) => {
        const error = errors.find(Boolean);
        return error ? `${error.constructor?.name ?? 'GPUError'}: ${error.message}` : null;
    });
}

// Watches `canvas` for WebGL2 context loss and restore. Calls `event.preventDefault()` on loss
// so the browser can restore the context, and caches the existing context handle once so
// `webglWatchLost` checks `isContextLost()` without calling `getContext` on every frame.
export function watchWebglContext(canvas, preventDefault, onLost, onRestored) {
    let gl = canvas.getContext('webgl2');
    let lost = !gl || gl.isContextLost();
    const handleLost = (event) => {
        if (preventDefault) event.preventDefault();
        lost = true;
        const detail = event.statusMessage ? `: ${event.statusMessage}` : '';
        onLost(`WebGL2 context lost${detail}`);
    };
    const handleRestored = () => {
        lost = false;
        gl = canvas.getContext('webgl2');
        onRestored();
    };
    canvas.addEventListener('webglcontextlost', handleLost);
    canvas.addEventListener('webglcontextrestored', handleRestored);
    return {
        isLost() { return lost || !gl || gl.isContextLost(); },
        dispose() {
            canvas.removeEventListener('webglcontextlost', handleLost);
            canvas.removeEventListener('webglcontextrestored', handleRestored);
        },
    };
}

export function webglWatchLost(handle) {
    return !!handle?.isLost?.();
}

export function disposeWebglWatch(handle) {
    handle?.dispose?.();
}
