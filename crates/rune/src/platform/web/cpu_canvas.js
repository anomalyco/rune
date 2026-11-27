// Batched BGRA->RGBA dirty-rect upload for the web CPU 2D canvas.

const LITTLE_ENDIAN = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

// Swizzles dirty rectangle `[x0, x0 + rw) x [y0, y0 + rh)` from BGRA rows in WASM memory
// (`bgraRows`, covering rows `[y0, y0 + rh)` at `canvasWidth` pixels per row) into a reusable
// JS-heap `ImageData` and uploads only that rectangle in one `putImageData` call.
//
// Returns the swizzle duration in milliseconds when `measure` is true, or `0` otherwise.
export function putCpuRect(ctx, imageData, bgraRows, canvasWidth, x0, y0, rw, rh, measure = false) {
    if (!ctx || !imageData || !bgraRows) return 0;
    if (rw <= 0 || rh <= 0 || canvasWidth <= 0) return 0;
    const dst = imageData.data;
    if (!dst || x0 < 0 || y0 < 0 || x0 + rw > imageData.width || y0 + rh > imageData.height) {
        return 0;
    }
    const needBytes = rh * canvasWidth * 4;
    if (bgraRows.byteLength < needBytes) return 0;

    const perf = measure ? globalThis.performance : null;
    const began = perf ? perf.now() : 0;

    const imgW = imageData.width;
    const aligned = LITTLE_ENDIAN && ((bgraRows.byteOffset & 3) === 0) && ((dst.byteOffset & 3) === 0);
    if (aligned) {
        const src32 = new Uint32Array(bgraRows.buffer, bgraRows.byteOffset, needBytes >>> 2);
        const dst32 = new Uint32Array(dst.buffer, dst.byteOffset, dst.byteLength >>> 2);
        if (x0 === 0 && rw === canvasWidth && canvasWidth === imgW) {
            const count = rw * rh;
            const dstBase = y0 * imgW;
            for (let i = 0; i < count; i++) {
                const v = src32[i];
                dst32[dstBase + i] = (v & 0xff00ff00) | ((v >>> 16) & 0xff) | ((v & 0xff) << 16);
            }
        } else {
            for (let row = 0; row < rh; row++) {
                let s = row * canvasWidth + x0;
                let d = (y0 + row) * imgW + x0;
                const end = s + rw;
                while (s < end) {
                    const v = src32[s++];
                    dst32[d++] = (v & 0xff00ff00) | ((v >>> 16) & 0xff) | ((v & 0xff) << 16);
                }
            }
        }
    } else {
        for (let row = 0; row < rh; row++) {
            let s = (row * canvasWidth + x0) * 4;
            let d = ((y0 + row) * imgW + x0) * 4;
            const end = s + rw * 4;
            while (s < end) {
                dst[d] = bgraRows[s + 2];
                dst[d + 1] = bgraRows[s + 1];
                dst[d + 2] = bgraRows[s];
                dst[d + 3] = bgraRows[s + 3];
                s += 4;
                d += 4;
            }
        }
    }

    const swizzleMs = perf ? perf.now() - began : 0;
    ctx.putImageData(imageData, 0, 0, x0, y0, rw, rh);
    return swizzleMs;
}
