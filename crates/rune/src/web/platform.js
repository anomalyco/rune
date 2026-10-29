// Plain text, with HTML and a PNG image (a Uint8Array) beside it when there are some (`html`
// is '' and `png` empty otherwise). Empty text isn't written beside another form.
export async function writeClipboard(text, html, png) {
    const clipboard = navigator.clipboard;
    if (!clipboard) throw new Error('Clipboard requires HTTPS.');
    if ((!html && !png.length) || typeof ClipboardItem === 'undefined') return clipboard.writeText(text);
    const forms = {};
    if (text) forms['text/plain'] = new Blob([text], { type: 'text/plain' });
    if (html) forms['text/html'] = new Blob([html], { type: 'text/html' });
    if (png.length) forms['image/png'] = new Blob([png], { type: 'image/png' });
    return clipboard.write([new ClipboardItem(forms)]);
}

// `{ text, html, png }`, each undefined when the clipboard lacks it; `html` only when `rich`,
// and `png` (a Uint8Array of at most `maxImage` bytes) only when `image`. `read()` starts
// before the first await, inside the input handler; where it fails, plain `readText()` is the
// fallback.
export async function readClipboard(rich, image, maxImage) {
    const clipboard = navigator.clipboard;
    if (!clipboard) throw new Error('Clipboard requires HTTPS.');
    if (rich && clipboard.read) {
        try {
            let text, html, png;
            for (const item of await clipboard.read()) {
                if (text === undefined && item.types.includes('text/plain')) text = await (await item.getType('text/plain')).text();
                if (html === undefined && item.types.includes('text/html')) html = await (await item.getType('text/html')).text();
                if (image && png === undefined && item.types.includes('image/png')) png = await pngBytes(await item.getType('image/png'), maxImage);
            }
            return { text, html, png };
        } catch {}
    }
    return { text: await clipboard.readText() };
}

// A PNG blob's bytes, or undefined when it's larger than `max`.
export async function pngBytes(blob, max) {
    return blob && blob.size <= max ? new Uint8Array(await blob.arrayBuffer()) : undefined;
}

export function openLink(url) {
    const parsed = new URL(url);
    if (!['https:', 'http:', 'mailto:'].includes(parsed.protocol)) throw new Error('Unsupported link scheme.');
    window.open(parsed.href, '_blank', 'noopener,noreferrer');
}

export function viewport(canvas, callback, embedded) {
    const probe = document.createElement('div');
    probe.style.cssText = 'position:fixed;visibility:hidden;pointer-events:none;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)';
    document.body.append(probe);
    const update = () => {
        if (!embedded) {
            const v = window.visualViewport;
            canvas.style.width = `${v?.width ?? innerWidth}px`;
            canvas.style.height = `${v?.height ?? innerHeight}px`;
            canvas.style.left = `${v?.offsetLeft ?? 0}px`;
            canvas.style.top = `${v?.offsetTop ?? 0}px`;
        }
        const s = getComputedStyle(probe);
        callback(JSON.stringify({ kind: 'viewport', insets: embedded ? [0, 0, 0, 0] : [s.paddingTop, s.paddingRight, s.paddingBottom, s.paddingLeft].map(parseFloat) }));
    };
    const motion = matchMedia('(prefers-reduced-motion: reduce)');
    const reduced = () => callback(JSON.stringify({ kind: 'motion', reduced: motion.matches }));
    const focus = () => callback(JSON.stringify({ kind: 'focus', focused: document.hasFocus() }));
    const visibility = () => callback(JSON.stringify({ kind: 'visibility', visible: !document.hidden }));
    let dprMedia = null;
    const watchDpr = () => {
        dprMedia?.removeEventListener('change', onDpr);
        dprMedia = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
        dprMedia.addEventListener('change', onDpr);
    };
    function onDpr() {
        watchDpr();
        callback(JSON.stringify({ kind: 'dpr', scale: window.devicePixelRatio || 1 }));
    }
    watchDpr();
    window.visualViewport?.addEventListener('resize', update);
    window.visualViewport?.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    motion.addEventListener('change', reduced);
    window.addEventListener('focus', focus);
    window.addEventListener('blur', focus);
    document.addEventListener('visibilitychange', visibility);
    // Mount is called while Rust is installing the shell; enter it only after installation.
    queueMicrotask(() => { update(); reduced(); focus(); visibility(); });
    return () => {
        dprMedia?.removeEventListener('change', onDpr);
        window.visualViewport?.removeEventListener('resize', update);
        window.visualViewport?.removeEventListener('scroll', update);
        window.removeEventListener('resize', update);
        motion.removeEventListener('change', reduced);
        window.removeEventListener('focus', focus);
        window.removeEventListener('blur', focus);
        document.removeEventListener('visibilitychange', visibility);
        probe.remove();
    };
}

// Decode a raster image (PNG, JPEG, WebP, GIF, AVIF) off the main thread with the browser's
// native codec, preserving straight alpha and EXIF orientation; returns `Uint8Array` headed by
// `[w_le32, h_le32, ...rgba8]`, or `undefined` when unavailable or unsupported.
export async function decodeImage(bytes) {
    if (typeof createImageBitmap !== 'function' || typeof OffscreenCanvas === 'undefined') return undefined;
    let bmp;
    try {
        bmp = await createImageBitmap(new Blob([bytes]), { premultiplyAlpha: 'none', colorSpaceConversion: 'default' });
        const { width: w, height: h } = bmp;
        if (w <= 0 || h <= 0 || w > 8192 || h > 8192 || w * h > 64 * 1024 * 1024) return undefined;
        const c = new OffscreenCanvas(w, h);
        const ctx = c.getContext('2d', { willReadFrequently: true });
        if (!ctx) return undefined;
        ctx.drawImage(bmp, 0, 0);
        const data = ctx.getImageData(0, 0, w, h).data;
        const out = new Uint8Array(8 + data.byteLength);
        const view = new DataView(out.buffer);
        view.setUint32(0, w, true);
        view.setUint32(4, h, true);
        out.set(data, 8);
        return out;
    } catch {
        return undefined;
    } finally {
        bmp?.close();
    }
}
