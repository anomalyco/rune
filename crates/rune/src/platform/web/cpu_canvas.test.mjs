import assert from 'node:assert/strict';
import test from 'node:test';
import { putCpuRect } from './cpu_canvas.js';

function makeImageData(width, height, fill = 0) {
    const data = new Uint8ClampedArray(width * height * 4);
    data.fill(fill);
    return { width, height, data };
}

function makeCtx() {
    const calls = [];
    return {
        calls,
        putImageData(imageData, dx, dy, dirtyX, dirtyY, dirtyW, dirtyH) {
            calls.push({ imageData, dx, dy, dirtyX, dirtyY, dirtyW, dirtyH });
        },
    };
}

test('putCpuRect swizzles a full-canvas BGRA buffer into RGBA in one putImageData call', () => {
    const w = 4;
    const h = 3;
    const bgra = new Uint8Array(w * h * 4);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const p = (y * w + x) * 4;
            bgra[p] = 10 + x;       // B
            bgra[p + 1] = 20 + y;   // G
            bgra[p + 2] = 200 + x;  // R
            bgra[p + 3] = 0xff;     // A (high bit set to verify unsigned shift)
        }
    }
    const image = makeImageData(w, h, 0xaa);
    const ctx = makeCtx();

    putCpuRect(ctx, image, bgra, w, 0, 0, w, h, false);

    assert.equal(ctx.calls.length, 1);
    assert.deepEqual(ctx.calls[0], {
        imageData: image,
        dx: 0,
        dy: 0,
        dirtyX: 0,
        dirtyY: 0,
        dirtyW: w,
        dirtyH: h,
    });
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const p = (y * w + x) * 4;
            assert.deepEqual(
                [image.data[p], image.data[p + 1], image.data[p + 2], image.data[p + 3]],
                [200 + x, 20 + y, 10 + x, 0xff],
            );
        }
    }
});

test('putCpuRect swizzles only the dirty sub-rectangle and leaves surrounding ImageData pixels untouched', () => {
    const w = 6;
    const h = 5;
    const x0 = 2;
    const y0 = 1;
    const rw = 3;
    const rh = 2;
    const fullBgra = new Uint8Array(w * h * 4);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const p = (y * w + x) * 4;
            fullBgra[p] = x * 10;       // B
            fullBgra[p + 1] = y * 20;   // G
            fullBgra[p + 2] = 100 + x;  // R
            fullBgra[p + 3] = 250;      // A
        }
    }
    // Pass only the row slice covering [y0, y0 + rh), matching Rust's &self.buffer[byte_start..byte_end].
    const bgraRows = fullBgra.subarray(y0 * w * 4, (y0 + rh) * w * 4);
    const image = makeImageData(w, h, 0x55);
    const ctx = makeCtx();

    putCpuRect(ctx, image, bgraRows, w, x0, y0, rw, rh, true);

    assert.equal(ctx.calls.length, 1);
    assert.deepEqual(ctx.calls[0], {
        imageData: image,
        dx: 0,
        dy: 0,
        dirtyX: x0,
        dirtyY: y0,
        dirtyW: rw,
        dirtyH: rh,
    });
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const p = (y * w + x) * 4;
            const inside = x >= x0 && x < x0 + rw && y >= y0 && y < y0 + rh;
            const got = [image.data[p], image.data[p + 1], image.data[p + 2], image.data[p + 3]];
            if (inside) {
                assert.deepEqual(got, [100 + x, y * 20, x * 10, 250], `pixel (${x},${y})`);
            } else {
                assert.deepEqual(got, [0x55, 0x55, 0x55, 0x55], `untouched pixel (${x},${y})`);
            }
        }
    }
});

test('putCpuRect handles unaligned Uint8Array byteOffset via the byte-loop fallback', () => {
    const w = 3;
    const h = 2;
    const backing = new ArrayBuffer(w * h * 4 + 3);
    const fullBgra = new Uint8Array(backing, 1, w * h * 4);
    // Pixel (1, 1): B=0x12, G=0x34, R=0x56, A=0xfe
    const srcAt = (1 * w + 1) * 4;
    fullBgra[srcAt] = 0x12;
    fullBgra[srcAt + 1] = 0x34;
    fullBgra[srcAt + 2] = 0x56;
    fullBgra[srcAt + 3] = 0xfe;
    const bgraRows = fullBgra.subarray(1 * w * 4, 2 * w * 4);

    const image = makeImageData(w, h, 0);
    const ctx = makeCtx();

    putCpuRect(ctx, image, bgraRows, w, 1, 1, 1, 1, false);

    assert.equal(ctx.calls.length, 1);
    const dstAt = (1 * w + 1) * 4;
    assert.deepEqual(
        [image.data[dstAt], image.data[dstAt + 1], image.data[dstAt + 2], image.data[dstAt + 3]],
        [0x56, 0x34, 0x12, 0xfe],
    );
});

test('putCpuRect ignores empty or out-of-bounds rectangles without calling putImageData', () => {
    const image = makeImageData(4, 4, 0);
    const bgra = new Uint8Array(4 * 4 * 4);
    const ctx = makeCtx();

    assert.equal(putCpuRect(ctx, image, bgra, 4, 0, 0, 0, 2), 0);
    assert.equal(putCpuRect(ctx, image, bgra, 4, 0, 0, 2, 0), 0);
    assert.equal(putCpuRect(ctx, image, bgra, 4, 3, 0, 2, 2), 0);
    assert.equal(putCpuRect(ctx, image, bgra.subarray(0, 4), 4, 0, 0, 4, 2), 0);
    assert.equal(ctx.calls.length, 0);
});
