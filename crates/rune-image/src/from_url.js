// `url` (relative to the page) decoded by the browser's own codecs, which run off the main
// thread: `[width, height, rgba]` with straight-alpha sRGB RGBA bytes, upright by EXIF. SVG comes
// back as `[0, 0, bytes]` for Rune to rasterize. Images over `maxSide` a side are refused
// before their pixels are read. It resolves in a task of its own, so Rune's work on one image
// (its mips) never runs in the same long task as another's or a frame's.
export async function fetchImage(url, maxSide) {
    const reply = await readImage(url, maxSide);
    await nextTask();
    return reply;
}

async function readImage(url, maxSide) {
    const response = await fetch(new URL(url, document.baseURI));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const blob = await response.blob();
    const type = (response.headers.get('content-type') || blob.type).split(';')[0].trim().toLowerCase();
    const path = response.url ? new URL(response.url).pathname : '';
    if (type === 'image/svg+xml' || /\.svgz?$/i.test(path)) return [0, 0, new Uint8Array(await blob.arrayBuffer())];
    return blobRgba(blob, maxSide);
}

export async function decodeImage(bytes, maxSide) {
    const reply = await blobRgba(new Blob([bytes]), maxSide);
    await nextTask();
    return reply;
}

async function blobRgba(blob, maxSide) {
    const bitmap = await createImageBitmap(blob, { imageOrientation: 'from-image', colorSpaceConversion: 'default' });
    try {
        const { width, height } = bitmap;
        if (width > maxSide || height > maxSide) throw new Error(`${width}×${height} is over ${maxSide} pixels a side`);
        const canvas = typeof OffscreenCanvas === 'function' ? new OffscreenCanvas(width, height)
            : Object.assign(document.createElement('canvas'), { width, height });
        const context = canvas.getContext('2d', { willReadFrequently: true });
        context.drawImage(bitmap, 0, 0);
        return [width, height, new Uint8Array(context.getImageData(0, 0, width, height).data.buffer)];
    } finally {
        bitmap.close();
    }
}

export function nextTask() {
    return globalThis.scheduler?.yield ? globalThis.scheduler.yield() : new Promise((resolve) => setTimeout(resolve, 0));
}
