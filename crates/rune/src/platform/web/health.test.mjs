import assert from 'node:assert/strict';
import test from 'node:test';
import { dispatchGpuFallback, isSoftwareDevice, restoreGpu, watchGpu } from './health.js';

test('restoreGpu clears foreign onuncapturederror without duplicating watchGpu listeners', async () => {
    let resolveLost;
    const listeners = [];
    const device = {
        onuncapturederror: null,
        addEventListener(type, fn) {
            if (type === 'uncapturederror') listeners.push(fn);
        },
        lost: new Promise((resolve) => { resolveLost = resolve; }),
        dispatchUncaptured(error) {
            let prevented = false;
            const event = { error, preventDefault() { prevented = true; } };
            for (const fn of listeners) fn(event);
            if (typeof this.onuncapturederror === 'function') this.onuncapturederror(event);
            return prevented;
        },
    };

    const runeErrors = [];
    const runeLost = [];
    watchGpu(
        device,
        (kind, msg) => runeErrors.push([kind, msg]),
        (reason, msg) => runeLost.push([reason, msg]),
    );

    const foreignErrors = [];
    device.onuncapturederror = (event) => foreignErrors.push(event.error.message);

    // Restoring clears the foreign single-slot handler and adds no duplicate listener.
    restoreGpu(device);
    restoreGpu(device);
    assert.equal(device.onuncapturederror, null);
    assert.equal(listeners.length, 1);

    const err = new Error('validation failed');
    const prevented = device.dispatchUncaptured(err);
    assert.equal(prevented, true);
    assert.deepEqual(runeErrors, [['Error', 'validation failed']]);
    assert.deepEqual(foreignErrors, []);

    resolveLost({ reason: 'unknown', message: 'driver reset' });
    await device.lost;
    assert.deepEqual(runeLost, [['unknown', 'driver reset']]);
});

test('restoreGpu tolerates null or objects without onuncapturederror', () => {
    assert.doesNotThrow(() => restoreGpu(null));
    assert.doesNotThrow(() => restoreGpu({}));
});

test('isSoftwareDevice detects fallback flag and software rasterizer metadata', () => {
    assert.equal(isSoftwareDevice(null), false);
    assert.equal(isSoftwareDevice({}), false);
    assert.equal(isSoftwareDevice({ adapterInfo: { vendor: 'apple', architecture: 'metal-3', isFallbackAdapter: false } }), false);
    assert.equal(isSoftwareDevice({ adapterInfo: { isFallbackAdapter: true } }), true);
    assert.equal(isSoftwareDevice({ adapterInfo: { vendor: 'google', description: 'Google SwiftShader' } }), true);
    assert.equal(isSoftwareDevice({ adapterInfo: { architecture: 'llvmpipe' } }), true);
    assert.equal(isSoftwareDevice({ adapterInfo: { device: 'Mesa Lavapipe' } }), true);
    assert.equal(isSoftwareDevice({ adapterInfo: { description: 'Microsoft Basic Render Driver' } }), true);
});

test('dispatchGpuFallback emits bubbling runegpufallback CustomEvent with stage and reason', () => {
    const dispatched = [];
    const canvas = { dispatchEvent(ev) { dispatched.push(ev); return true; } };
    dispatchGpuFallback(canvas, 'init', 'software/fallback GPU adapter rejected');
    assert.equal(dispatched.length, 1);
    assert.equal(dispatched[0].type, 'runegpufallback');
    assert.equal(dispatched[0].bubbles, true);
    assert.deepEqual(dispatched[0].detail, {
        stage: 'init',
        reason: 'software/fallback GPU adapter rejected',
    });
    assert.doesNotThrow(() => dispatchGpuFallback(null, 'device_lost', 'lost'));
});
