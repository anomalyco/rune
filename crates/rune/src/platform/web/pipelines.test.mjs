import assert from 'node:assert/strict';
import test from 'node:test';
import { canDivert, compilesAsync, divert, settle } from './pipelines.js';

function withGlobals(overrides, fn) {
    const saved = new Map();
    for (const key of Object.keys(overrides)) {
        saved.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
        if (overrides[key] === undefined) {
            delete globalThis[key];
        } else {
            Object.defineProperty(globalThis, key, {
                value: overrides[key],
                configurable: true,
                writable: true,
            });
        }
    }
    try {
        return fn();
    } finally {
        for (const [key, desc] of saved) {
            if (desc === undefined) delete globalThis[key];
            else Object.defineProperty(globalThis, key, desc);
        }
    }
}

test('compilesAsync requires navigator.gpu.requestAdapter and GPUDevice.prototype.createRenderPipelineAsync', () => {
    class FakeGpuDevice {}
    FakeGpuDevice.prototype.createRenderPipelineAsync = () => Promise.resolve({});

    // Missing navigator.gpu (WebGL2-only or insecure origin): must wait for QUIET.
    withGlobals({ navigator: {}, GPUDevice: FakeGpuDevice }, () => {
        assert.equal(compilesAsync(), false);
    });
    withGlobals({ navigator: { gpu: null }, GPUDevice: FakeGpuDevice }, () => {
        assert.equal(compilesAsync(), false);
    });

    // Stub navigator.gpu without requestAdapter: cannot probe WebGPU.
    withGlobals({ navigator: { gpu: {} }, GPUDevice: FakeGpuDevice }, () => {
        assert.equal(compilesAsync(), false);
    });

    // navigator.gpu present, but WebGPU lacks createRenderPipelineAsync: must wait for QUIET.
    class SyncGpuDevice {}
    withGlobals({ navigator: { gpu: { requestAdapter: async () => ({}) } }, GPUDevice: SyncGpuDevice }, () => {
        assert.equal(compilesAsync(), false);
    });

    // Full WebGPU surface: safe to probe WebGPU early (adapter presence is checked asynchronously;
    // fallback to WebGL2 is still gated on QUIET in Rust).
    withGlobals({ navigator: { gpu: { requestAdapter: async () => ({}) } }, GPUDevice: FakeGpuDevice }, () => {
        assert.equal(compilesAsync(), true);
    });
    withGlobals({ navigator: { gpu: { requestAdapter: async () => null } }, GPUDevice: FakeGpuDevice }, () => {
        assert.equal(compilesAsync(), true);
    });
});

test('canDivert and divert/settle redirect createRenderPipeline to createRenderPipelineAsync', async () => {
    assert.equal(canDivert(null), false);
    assert.equal(canDivert({}), false);

    const asyncCalls = [];
    const syncCalls = [];
    class FakeGpuDevice {
        createShaderModule(desc) { return desc; }
        createRenderPipeline(desc) {
            syncCalls.push(desc);
            return { standIn: true };
        }
        createRenderPipelineAsync(desc) {
            asyncCalls.push(desc);
            return Promise.resolve({ asyncPipeline: desc.label });
        }
    }

    await withGlobals({ GPUDevice: FakeGpuDevice }, async () => {
        const device = new FakeGpuDevice();
        assert.equal(canDivert(device), true);

        const diverted = divert(device);
        const first = device.createRenderPipeline({ label: 'uber' });
        const second = device.createRenderPipeline({ label: 'fill' });
        assert.equal(first, second);
        assert.equal(syncCalls.length, 1);
        assert.deepEqual(asyncCalls.map(d => d.label), ['uber', 'fill']);

        const settled = await settle(diverted);
        assert.deepEqual(settled, [{ asyncPipeline: 'uber' }, { asyncPipeline: 'fill' }]);
        assert.equal(Object.hasOwn(device, 'createRenderPipeline'), false);
    });
});
