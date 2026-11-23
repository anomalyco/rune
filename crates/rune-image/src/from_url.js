// Fetch `url` (relative to the page) as `[isSvg, mimeType, bytes]` so Rust can validate
// declared image dimensions before the browser decodes its raster.
export async function fetchBytes(url) {
    const response = await fetch(new URL(url, document.baseURI));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const type = (response.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
    const path = response.url ? new URL(response.url).pathname : '';
    const bytes = new Uint8Array(await response.arrayBuffer());
    return [type === 'image/svg+xml' || /\.svgz?$/i.test(path), type, bytes];
}

// Encoded image bytes decoded by the browser's own codecs, which run off the main thread:
// `[width, height, rgba]` with straight-alpha sRGB RGBA bytes, upright by EXIF. Images over
// `maxSide` a side are refused again after decode as a second guard. It resolves in a task of
// its own, so Rune's work on one image (its mips) never runs in the same long task as another's
// or a frame's.
export async function decodeImage(bytes, mime, maxSide) {
    const blob = mime ? new Blob([bytes], { type: mime }) : new Blob([bytes]);
    const reply = await blobRgba(blob, maxSide);
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
