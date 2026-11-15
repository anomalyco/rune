// The renderer's pipelines compiled off the browser's GPU thread before the renderer is made.
// While diverted, the device's createRenderPipeline starts createRenderPipelineAsync with the
// same descriptor and answers at once with a small stand-in, so a renderer made meanwhile costs
// nothing to compile and is thrown away. Once the async pipelines are in, the real renderer's
// identical descriptors find them in the browser's pipeline cache (Dawn keeps one pipeline per
// descriptor while one is alive), so the hand-over no longer holds the GPU thread that also
// presents the CPU canvas's frames.

// Whether this browser's WebGPU has an async pipeline compile, before there's a device.
export function compilesAsync() {
    return typeof globalThis.GPUDevice?.prototype?.createRenderPipelineAsync === 'function';
}

// Whether `device` compiles pipelines asynchronously.
export function canDivert(device) {
    return typeof device?.createRenderPipelineAsync === 'function';
}

// Starts diverting `device`'s createRenderPipeline; `settle` ends it.
export function divert(device) {
    const started = [];
    let standIn = null;
    device.createRenderPipeline = (descriptor) => {
        started.push(device.createRenderPipelineAsync(descriptor).catch(() => null));
        standIn ??= GPUDevice.prototype.createRenderPipeline.call(device, standInDescriptor(device));
        return standIn;
    };
    return { device, started };
}

// Ends the diversion, and resolves to the compiled pipelines (to keep alive until the real
// renderer has made its own) once all of them are in.
export function settle(diverted) {
    delete diverted.device.createRenderPipeline;
    return Promise.all(diverted.started);
}

function standInDescriptor(device) {
    const module = device.createShaderModule({ code: '@vertex fn v() -> @builtin(position) vec4f { return vec4f(); } @fragment fn f() -> @location(0) vec4f { return vec4f(); }' });
    return { layout: 'auto', vertex: { module, entryPoint: 'v' }, fragment: { module, entryPoint: 'f', targets: [{ format: 'rgba8unorm' }] } };
}
