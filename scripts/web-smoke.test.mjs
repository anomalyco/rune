import assert from 'node:assert/strict';
import test from 'node:test';
import { deflateSync } from 'node:zlib';
import { chromiumArgs, inspectPngUniformity, assertRenderedPng } from './web-smoke.mjs';

const CRC_TABLE = (() => {
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
        table[n] = c >>> 0;
    }
    return table;
})();

function crc32(buf) {
    let c = 0xffffffff;
    for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
    const typeBytes = Buffer.from(type, 'ascii');
    const out = Buffer.alloc(12 + data.length);
    out.writeUInt32BE(data.length, 0);
    typeBytes.copy(out, 4);
    data.copy(out, 8);
    out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
    return out;
}

function paeth(a, b, c) {
    const p = a + b - c;
    const pa = Math.abs(p - a);
    const pb = Math.abs(p - b);
    const pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

function encodePng({ width, height, colorType = 6, bitDepth = 8, rows, filterType = 0, palette = null, splitIdat = false }) {
    const channels = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[colorType];
    const bpp = Math.max(1, Math.ceil((channels * bitDepth) / 8));
    const stride = Math.ceil((width * channels * bitDepth) / 8);
    const raw = Buffer.alloc(height * (1 + stride));
    const zero = new Uint8Array(stride);
    for (let y = 0; y < height; y++) {
        const curr = rows[y];
        const prev = y > 0 ? rows[y - 1] : zero;
        const ft = typeof filterType === 'function' ? filterType(y) : filterType;
        const dst = y * (1 + stride);
        raw[dst] = ft;
        for (let x = 0; x < stride; x++) {
            const a = x >= bpp ? curr[x - bpp] : 0;
            const b = prev[x];
            const c = x >= bpp ? prev[x - bpp] : 0;
            let pred = 0;
            if (ft === 1) pred = a;
            else if (ft === 2) pred = b;
            else if (ft === 3) pred = (a + b) >> 1;
            else if (ft === 4) pred = paeth(a, b, c);
            raw[dst + 1 + x] = (curr[x] - pred) & 0xff;
        }
    }
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(width, 0);
    ihdr.writeUInt32BE(height, 4);
    ihdr[8] = bitDepth;
    ihdr[9] = colorType;
    const chunks = [
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        pngChunk('IHDR', ihdr),
    ];
    if (palette) chunks.push(pngChunk('PLTE', Buffer.from(palette)));
    const compressed = deflateSync(raw);
    if (splitIdat && compressed.length > 4) {
        const mid = Math.floor(compressed.length / 2);
        chunks.push(pngChunk('IDAT', compressed.subarray(0, mid)));
        chunks.push(pngChunk('IDAT', compressed.subarray(mid)));
    } else {
        chunks.push(pngChunk('IDAT', compressed));
    }
    chunks.push(pngChunk('IEND', Buffer.alloc(0)));
    return Buffer.concat(chunks);
}

function solidRows(width, height, pixel) {
    const row = new Uint8Array(width * pixel.length);
    for (let x = 0; x < width; x++) row.set(pixel, x * pixel.length);
    return Array.from({ length: height }, () => new Uint8Array(row));
}

test('chromiumArgs enables SwiftShader WebGPU on Linux without changing macOS or Windows', () => {
    const linux = chromiumArgs('linux');
    assert.ok(linux.includes('--enable-unsafe-webgpu'));
    assert.ok(linux.includes('--enable-unsafe-swiftshader'));
    assert.ok(linux.includes('--enable-features=Vulkan'));
    assert.ok(linux.includes('--use-vulkan=swiftshader'));
    assert.ok(linux.includes('--use-angle=swiftshader'));
    assert.ok(linux.includes('--use-webgpu-adapter=swiftshader'));
    assert.ok(linux.includes('--ignore-gpu-blocklist'));
    assert.ok(!linux.includes('--use-angle=metal'));

    assert.deepEqual(chromiumArgs('darwin'), ['--enable-unsafe-webgpu', '--use-angle=metal']);
    assert.deepEqual(chromiumArgs('win32'), ['--enable-unsafe-webgpu']);
});

test('inspectPngUniformity flags solid-color screenshots across all PNG filter types', () => {
    for (const filterType of [0, 1, 2, 3, 4]) {
        const dark = encodePng({
            width: 16,
            height: 12,
            colorType: 6,
            rows: solidRows(16, 12, [18, 18, 18, 255]),
            filterType,
            splitIdat: filterType === 4,
        });
        assert.deepEqual(inspectPngUniformity(dark), {
            uniform: true,
            width: 16,
            height: 12,
            color: [18, 18, 18, 255],
        });
        assert.throws(
            () => assertRenderedPng(dark, 'web-dark.png'),
            /web-dark.png.*18, 18, 18, 255/,
        );
    }

    const blackRgb = encodePng({
        width: 8,
        height: 8,
        colorType: 2,
        rows: solidRows(8, 8, [0, 0, 0]),
        filterType: 4,
    });
    assert.deepEqual(inspectPngUniformity(blackRgb), {
        uniform: true,
        width: 8,
        height: 8,
        color: [0, 0, 0],
    });
    assert.throws(() => assertRenderedPng(blackRgb, 'web-light-keyboard-size.png'));
});

test('inspectPngUniformity accepts screenshots with drawn pixels across all PNG filter types', () => {
    for (const filterType of [0, 1, 2, 3, 4]) {
        const rows = solidRows(16, 12, [18, 18, 18, 255]);
        // Put a single differing pixel in the bottom-right corner to verify full-image scan.
        rows[11].set([240, 240, 240, 255], 15 * 4);
        const png = encodePng({ width: 16, height: 12, colorType: 6, rows, filterType });
        assert.deepEqual(inspectPngUniformity(png), { uniform: false, width: 16, height: 12 });
        assert.doesNotThrow(() => assertRenderedPng(png, 'web-dark.png'));
    }

    // Exercise mixed per-row filters on a multi-color image.
    const rows = Array.from({ length: 10 }, (_, y) => {
        const row = new Uint8Array(10 * 4);
        for (let x = 0; x < 10; x++) row.set([x * 20, y * 20, 128, 255], x * 4);
        return row;
    });
    const gradient = encodePng({ width: 10, height: 10, colorType: 6, rows, filterType: y => y % 5 });
    assert.equal(inspectPngUniformity(gradient).uniform, false);
});

test('inspectPngUniformity handles indexed, 1-bit, and 16-bit PNGs and rejects corrupt input', () => {
    // Indexed PNG where two palette indices map to the same RGB color is still uniform.
    const uniformIndexed = encodePng({
        width: 4,
        height: 2,
        colorType: 3,
        bitDepth: 8,
        palette: [18, 18, 18, 18, 18, 18],
        rows: [new Uint8Array([0, 1, 0, 1]), new Uint8Array([1, 0, 1, 0])],
    });
    assert.equal(inspectPngUniformity(uniformIndexed).uniform, true);

    const distinctIndexed = encodePng({
        width: 4,
        height: 2,
        colorType: 3,
        bitDepth: 8,
        palette: [18, 18, 18, 255, 255, 255],
        rows: [new Uint8Array([0, 0, 0, 0]), new Uint8Array([0, 0, 0, 1])],
    });
    assert.equal(inspectPngUniformity(distinctIndexed).uniform, false);

    // 1-bit grayscale with non-byte-aligned width (padding bits must be ignored).
    const uniform1bit = encodePng({
        width: 3,
        height: 2,
        colorType: 0,
        bitDepth: 1,
        rows: [new Uint8Array([0b11100000]), new Uint8Array([0b11111111])],
    });
    assert.equal(inspectPngUniformity(uniform1bit).uniform, true);

    // 16-bit RGB solid color.
    const solid16 = encodePng({
        width: 2,
        height: 2,
        colorType: 2,
        bitDepth: 16,
        rows: solidRows(2, 2, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06]),
    });
    assert.deepEqual(inspectPngUniformity(solid16), {
        uniform: true,
        width: 2,
        height: 2,
        color: [0x0102, 0x0304, 0x0506],
    });

    assert.throws(() => inspectPngUniformity(Buffer.from('not a png')));
});
