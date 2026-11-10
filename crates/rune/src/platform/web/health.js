// A WebGPU device's errors and loss, read from the JS device itself. wgpu's own handlers turn a
// GPUInternalError into a panic (an `unreachable` trap that ends the app), so Rune never asks
// wgpu for them on the web.

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
export function watchGpu(device, onError, onLost) {
    device.addEventListener('uncapturederror', (event) => {
        event.preventDefault();
        onError(event.error?.constructor?.name ?? 'GPUError', event.error?.message ?? '');
    });
    device.lost.then((info) => {
        if (info.reason !== 'destroyed') onLost(info.reason ?? 'unknown', info.message ?? '');
    });
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
