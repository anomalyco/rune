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

// Shared PNG encoder source used both inside a Blob Web Worker (off main thread) and as a
// time-sliced CompressionStream fallback when a strict CSP blocks blob: workers. Matches
// `crates/rune-image/src/mips.rs::straight_rgba` (linear-premultiplied mip 0 -> straight sRGB
// RGBA8) and `crates/rune/src/drag_out/mod.rs::encode_png` (8-bit RGBA, Perceptual sRGB chunk).
function pngEncoderCore() {
    const CRC_TABLE = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
        CRC_TABLE[n] = c >>> 0;
    }
    function crc32(parts) {
        let c = 0xffffffff;
        for (let p = 0; p < parts.length; p++) {
            const b = parts[p];
            for (let i = 0; i < b.length; i++) c = CRC_TABLE[(c ^ b[i]) & 0xff] ^ (c >>> 8);
        }
        return (c ^ 0xffffffff) >>> 0;
    }
    let srgbTables = null;
    function getSrgbTables() {
        if (srgbTables) return srgbTables;
        const STEPS = 4096;
        const dec = (s) => (s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4));
        const decode = new Float32Array(256);
        const up = new Float32Array(256);
        for (let i = 0; i < 256; i++) {
            decode[i] = dec(i / 255);
            up[i] = i === 255 ? Infinity : dec((i + 0.5) / 255);
        }
        const coarse = new Uint8Array(STEPS + 1);
        for (let i = 0; i <= STEPS; i++) {
            const t = i / STEPS;
            let lo = 0, hi = 256;
            while (lo < hi) {
                const mid = (lo + hi) >>> 1;
                if (up[mid] <= t) lo = mid + 1; else hi = mid;
            }
            coarse[i] = lo;
        }
        srgbTables = { decode, up, coarse, STEPS };
        return srgbTables;
    }
    function unpremulRow(src, srcOff, rowBytes, dst) {
        const { decode, up, coarse, STEPS } = getSrgbTables();
        for (let i = 0; i < rowBytes; i += 4) {
            const a = src[srcOff + i + 3];
            if (a === 255) {
                dst[i] = src[srcOff + i];
                dst[i + 1] = src[srcOff + i + 1];
                dst[i + 2] = src[srcOff + i + 2];
                dst[i + 3] = 255;
            } else if (a === 0) {
                dst[i] = 0; dst[i + 1] = 0; dst[i + 2] = 0; dst[i + 3] = 0;
            } else {
                const invA = 255 / a;
                for (let c = 0; c < 3; c++) {
                    let v = decode[src[srcOff + i + c]] * invA;
                    if (v < 0) v = 0; else if (v > 1) v = 1;
                    const b = coarse[(v * STEPS) | 0];
                    dst[i + c] = b + (v >= up[b] ? 1 : 0);
                }
                dst[i + 3] = a;
            }
        }
    }
    function writeU32(arr, off, v) {
        arr[off] = (v >>> 24) & 0xff;
        arr[off + 1] = (v >>> 16) & 0xff;
        arr[off + 2] = (v >>> 8) & 0xff;
        arr[off + 3] = v & 0xff;
    }
    function makeChunk(typeStr, data) {
        const type = new Uint8Array([typeStr.charCodeAt(0), typeStr.charCodeAt(1), typeStr.charCodeAt(2), typeStr.charCodeAt(3)]);
        const out = new Uint8Array(12 + data.length);
        writeU32(out, 0, data.length);
        out.set(type, 4);
        out.set(data, 8);
        writeU32(out, 8 + data.length, crc32([type, data]));
        return out;
    }
    async function encodePng(width, height, rgba, premul, maxBytes, yieldBudgetMs) {
        const rowBytes = width * 4;
        if (width <= 0 || height <= 0 || rgba.byteLength !== rowBytes * height) {
            return { code: 'failed', error: 'invalid RGBA dimensions' };
        }
        // Fixed PNG overhead: 8 sig + 25 IHDR + 13 sRGB + 12 IDAT header/crc + 12 IEND = 70 B.
        const OVERHEAD = 70;
        if (maxBytes <= OVERHEAD) return { code: 'too_large', error: 'image exceeds clipboard size limits' };
        const cs = new CompressionStream('deflate');
        const writer = cs.writable.getWriter();
        const reader = cs.readable.getReader();
        const idatParts = [];
        let idatLen = 0;
        let tooLarge = false;
        const readDone = (async () => {
            while (true) {
                const { value, done } = await reader.read();
                if (done) break;
                idatLen += value.byteLength;
                if (idatLen + OVERHEAD > maxBytes) {
                    tooLarge = true;
                    await reader.cancel();
                    break;
                }
                idatParts.push(value);
            }
        })();
        const stripRows = Math.max(1, Math.min(height, Math.floor((256 * 1024) / (rowBytes + 1)) || 1));
        const straightBuf = premul ? new Uint8Array(rowBytes) : null;
        let sliceStart = yieldBudgetMs > 0 ? performance.now() : 0;
        try {
            for (let y = 0; y < height && !tooLarge; y += stripRows) {
                const rows = Math.min(stripRows, height - y);
                const strip = new Uint8Array(rows * (rowBytes + 1));
                for (let r = 0; r < rows; r++) {
                    const srcOff = (y + r) * rowBytes;
                    let src = rgba;
                    let off = srcOff;
                    if (premul) {
                        unpremulRow(rgba, srcOff, rowBytes, straightBuf);
                        src = straightBuf;
                        off = 0;
                    }
                    const dstOff = r * (rowBytes + 1);
                    strip[dstOff] = 1; // PNG Sub filter
                    strip[dstOff + 1] = src[off];
                    strip[dstOff + 2] = src[off + 1];
                    strip[dstOff + 3] = src[off + 2];
                    strip[dstOff + 4] = src[off + 3];
                    for (let i = 4; i < rowBytes; i++) {
                        strip[dstOff + 1 + i] = (src[off + i] - src[off + i - 4]) & 0xff;
                    }
                }
                await writer.write(strip);
                if (yieldBudgetMs > 0 && performance.now() - sliceStart >= yieldBudgetMs) {
                    await new Promise((res) => setTimeout(res, 0));
                    sliceStart = performance.now();
                }
            }
            if (tooLarge) { await writer.abort().catch(() => {}); } else { await writer.close(); }
        } catch (e) {
            if (!tooLarge) throw e;
        }
        await readDone;
        if (tooLarge) return { code: 'too_large', error: 'image exceeds clipboard size limits' };
        const ihdrData = new Uint8Array(13);
        writeU32(ihdrData, 0, width);
        writeU32(ihdrData, 4, height);
        ihdrData[8] = 8; ihdrData[9] = 6; // 8-bit RGBA
        const sig = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
        const ihdr = makeChunk('IHDR', ihdrData);
        const srgb = makeChunk('sRGB', new Uint8Array([0]));
        const iend = new Uint8Array([0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82]);
        const idatType = new Uint8Array([0x49, 0x44, 0x41, 0x54]);
        const total = sig.length + ihdr.length + srgb.length + 12 + idatLen + iend.length;
        if (total > maxBytes) return { code: 'too_large', error: 'image exceeds clipboard size limits' };
        const png = new Uint8Array(total);
        let pos = 0;
        png.set(sig, pos); pos += sig.length;
        png.set(ihdr, pos); pos += ihdr.length;
        png.set(srgb, pos); pos += srgb.length;
        writeU32(png, pos, idatLen); pos += 4;
        png.set(idatType, pos); pos += 4;
        const idatStart = pos - 4;
        for (let i = 0; i < idatParts.length; i++) {
            png.set(idatParts[i], pos);
            pos += idatParts[i].byteLength;
        }
        writeU32(png, pos, crc32([png.subarray(idatStart, pos)])); pos += 4;
        png.set(iend, pos);
        return { png };
    }
    return { encodePng };
}

let pngCore = null;
let pngWorker = null;
let pngWorkerBroken = false;
let pngWorkerSeq = 0;
const pngWorkerPending = new Map();

function getPngWorker() {
    if (pngWorker) return pngWorker;
    if (pngWorkerBroken || typeof Worker !== 'function' || typeof Blob !== 'function' || typeof URL === 'undefined' || typeof URL.createObjectURL !== 'function') {
        return null;
    }
    try {
        const src = `const core = (${pngEncoderCore.toString()})();\nself.onmessage = async (ev) => {\n  const { id, width, height, buf, premul, maxBytes } = ev.data;\n  try {\n    const res = await core.encodePng(width, height, new Uint8Array(buf), premul, maxBytes, 0);\n    if (res.png) { self.postMessage({ id, png: res.png.buffer }, [res.png.buffer]); }\n    else { self.postMessage({ id, code: res.code, error: res.error }); }\n  } catch (e) {\n    self.postMessage({ id, code: "failed", error: String(e && e.message || e) });\n  }\n};`;
        const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
        const w = new Worker(url);
        URL.revokeObjectURL(url);
        w.onmessage = (ev) => {
            const msg = ev.data;
            const entry = pngWorkerPending.get(msg.id);
            if (!entry) return;
            pngWorkerPending.delete(msg.id);
            entry.resolve(msg.png ? { png: new Uint8Array(msg.png) } : { code: msg.code || 'failed', error: msg.error || 'PNG encode failed' });
        };
        w.onerror = () => {
            pngWorkerBroken = true;
            pngWorker = null;
            try { w.terminate(); } catch {}
            for (const entry of pngWorkerPending.values()) entry.resolve({ fallback: true });
            pngWorkerPending.clear();
        };
        pngWorker = w;
        return w;
    } catch {
        pngWorkerBroken = true;
        return null;
    }
}

// Encodes straight or linear-premultiplied mip-0 RGBA8 as PNG off the main thread using a
// Blob Web Worker + CompressionStream('deflate'), falling back to time-sliced async
// CompressionStream if Workers are unavailable or blocked by CSP.
export async function encodePngAsync(width, height, bytes, premul, maxBytes) {
    const w = getPngWorker();
    if (w) {
        const id = ++pngWorkerSeq;
        // Copy only if bytes is a view into a larger backing buffer (e.g. WASM linear memory).
        const owned = (bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength) ? bytes : bytes.slice();
        // Keep a fallback copy only if we have not yet confirmed the worker accepts messages (CSP
        // can reject blob: workers asynchronously via onerror on the first message).
        const res = await new Promise((resolve) => {
            pngWorkerPending.set(id, { resolve });
            w.postMessage({ id, width, height, buf: owned.buffer, premul, maxBytes }, [owned.buffer]);
        });
        if (!res.fallback) return res;
        return { fallback: true };
    }
    if (!pngCore) pngCore = pngEncoderCore();
    return pngCore.encodePng(width, height, bytes, premul, maxBytes, 6);
}

// Writes a PNG image inside the active user gesture by creating a `ClipboardItem` holding a
// `Promise<Blob>` synchronously and encoding the PNG off the main thread (Web Worker +
// CompressionStream('deflate') on Chrome 80+, Firefox 113+, Safari 16.4+; time-sliced
// CompressionStream when CSP blocks blob: workers; legacy main-thread Rust encoder where
// CompressionStream is absent).
export function writeClipboardImage(extract, checkOutcome, maxBytes) {
    const clipboard = navigator.clipboard;
    if (!clipboard || typeof clipboard.write !== 'function') {
        return Promise.reject(new Error('Clipboard image write requires HTTPS and Clipboard.write.'));
    }
    if (typeof ClipboardItem === 'undefined') {
        return Promise.reject(new Error('ClipboardItem is unavailable in this browser.'));
    }
    // Cancel the deferred encode if `clipboard.write` settles early (e.g. NotAllowedError)
    // so Rust wasm_bindgen closures dropped on settlement are never invoked afterward.
    let settled = false;
    let timerId = null;
    const blobPromise = new Promise((resolve, reject) => {
        timerId = setTimeout(async () => {
            timerId = null;
            if (settled) { reject(new Error('clipboard write settled')); return; }
            try {
                const hasStream = typeof CompressionStream === 'function';
                const out = extract(hasStream);
                if (!out || out.error) {
                    reject(new Error((out && out.error) || 'Image copy aborted.'));
                    return;
                }
                let png;
                if (out.png && out.png.length) {
                    png = out.png;
                } else if (out.bytes && out.width && out.height) {
                    let encoded = await encodePngAsync(out.width, out.height, out.bytes, !!out.premul, maxBytes);
                    if (settled) { reject(new Error('clipboard write settled')); return; }
                    if (encoded.fallback) {
                        // First-message CSP worker error neutered the transferred buffer; re-extract and use time-sliced stream.
                        const retry = extract(true);
                        if (!retry || retry.error || !retry.bytes) {
                            reject(new Error((retry && retry.error) || 'Image copy aborted.'));
                            return;
                        }
                        if (!pngCore) pngCore = pngEncoderCore();
                        encoded = await pngCore.encodePng(retry.width, retry.height, retry.bytes, !!retry.premul, maxBytes, 6);
                    }
                    if (settled) { reject(new Error('clipboard write settled')); return; }
                    if (!encoded.png) {
                        checkOutcome(encoded.code || 'failed');
                        reject(new Error(encoded.error || 'PNG encode failed.'));
                        return;
                    }
                    png = encoded.png;
                } else {
                    reject(new Error('Image copy aborted.'));
                    return;
                }
                if (settled) { reject(new Error('clipboard write settled')); return; }
                if (!checkOutcome('')) {
                    reject(new Error('copy superseded'));
                    return;
                }
                resolve(new Blob([png], { type: 'image/png' }));
            } catch (e) {
                reject(e);
            }
        }, 0);
    });
    const cancel = () => {
        settled = true;
        if (timerId !== null) { clearTimeout(timerId); timerId = null; }
        blobPromise.catch(() => {});
    };
    try {
        const writePromise = clipboard.write([new ClipboardItem({ 'image/png': blobPromise })]);
        return writePromise.then(
            (v) => { cancel(); return v; },
            (e) => { cancel(); throw e; },
        );
    } catch (e) {
        cancel();
        return Promise.reject(e);
    }
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

// Opens `url` in a new tab with no opener or referrer. False when no click or key press was
// active, which browsers block: with `noopener` the browser returns null whether or not the
// tab opened, so the page's user activation, read before `window.open` spends it, is all there
// is to go on. Browsers without `navigator.userActivation` say true.
export function openLink(url) {
    const parsed = new URL(url);
    if (!['https:', 'http:', 'mailto:'].includes(parsed.protocol)) throw new Error('Unsupported link scheme.');
    const activation = navigator.userActivation;
    const active = !activation || activation.isActive;
    window.open(parsed.href, '_blank', 'noopener,noreferrer');
    return active;
}

// `url` resolved against the page, and whether it stays on the page's origin.
export function resolveLink(url) {
    const parsed = new URL(url, document.baseURI);
    if (!['https:', 'http:', 'mailto:'].includes(parsed.protocol)) throw new Error('Unsupported link scheme.');
    return [parsed.href, parsed.origin === location.origin];
}

export function assignLink(href) {
    location.assign(href);
}

export function objectUrl(bytes, type) {
    return URL.createObjectURL(new Blob([bytes], { type }));
}

export function revokeUrl(url) {
    URL.revokeObjectURL(url);
}

// Sets `canvas`'s CSS cursor, or clears it when `css` is empty. Safari reads `image-set()` in a
// cursor only with its `-webkit-` prefix.
export function setCursor(canvas, css) {
    if (!css) return void canvas.style.removeProperty('cursor');
    canvas.style.cursor = css;
    if (!canvas.style.cursor) canvas.style.cursor = `-webkit-${css}`;
}

// Resolves in the page's next idle time, or after `timeout` ms at the latest.
export function nextIdle(timeout) {
    return new Promise((resolve) => globalThis.requestIdleCallback ? requestIdleCallback(() => resolve(), { timeout }) : setTimeout(resolve, timeout));
}

export async function fetchBytes(url) {
    const response = await fetch(new URL(url, document.baseURI));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}

export function resolveUrl(url, base) {
    return new URL(url, base).href;
}

// Browser preferred languages as BCP 47 tags, most preferred first (`Cx::system_languages`).
export function preferredLanguages() {
    const nav = globalThis.navigator;
    if (!nav) return [];
    const raw = nav.languages?.length ? [...nav.languages] : (nav.language ? [nav.language] : []);
    return raw.filter(tag => typeof tag === 'string' && tag.trim().length > 0);
}

export function viewport(canvas, callback, embedded) {
    const probe = document.createElement('div');
    probe.style.cssText = 'position:fixed;visibility:hidden;pointer-events:none;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)';
    document.body.append(probe);
    // The largest viewport: on iOS Safari `100lvh` is the viewport with the toolbar collapsed, so
    // a canvas or band sized from it doesn't change as the toolbar comes and goes mid-scroll.
    const largest = document.createElement('div');
    largest.style.cssText = 'position:fixed;top:0;left:0;width:0;height:100lvh;visibility:hidden;pointer-events:none';
    document.body.append(largest);
    // Full-window canvases keep their layout-viewport CSS box (`position:fixed;inset:0;width:100%;height:100%`)
    // instead of shrinking to `window.visualViewport`: iOS Safari and Chrome on Android (since
    // Chrome 108's default `interactive-widget=resizes-visual`) resize only the visual viewport
    // when the virtual keyboard opens. Leaving the canvas at the layout viewport prevents squash
    // distortion and spurious relayouts; instead report the unzoomed visual viewport's vertical
    // band so Rune can scroll or shift only the focused field into view.
    const update = () => {
        const s = getComputedStyle(probe);
        const layoutH = Number.isFinite(innerHeight) && innerHeight > 0 ? innerHeight : 0;
        const v = window.visualViewport;
        const zoomed = v && typeof v.scale === 'number' && v.scale > 1.01;
        let visual = null;
        if (!embedded && v && !zoomed && layoutH > 0) {
            const top = typeof v.offsetTop === 'number' && Number.isFinite(v.offsetTop) ? Math.max(0, v.offsetTop) : 0;
            const vh = typeof v.height === 'number' && Number.isFinite(v.height) && v.height > 0 ? v.height : layoutH;
            const bottom = Math.min(layoutH, top + vh);
            if (bottom > top) visual = [top, bottom, layoutH];
        }
        callback(JSON.stringify({ kind: 'viewport', h: largest.offsetHeight || innerHeight,
            insets: embedded ? [0, 0, 0, 0] : [s.paddingTop, s.paddingRight, s.paddingBottom, s.paddingLeft].map(parseFloat),
            visual }));
    };
    const motion = matchMedia('(prefers-reduced-motion: reduce)');
    const reduced = () => callback(JSON.stringify({ kind: 'motion', reduced: motion.matches }));
    // A primary pointer that can't hover (a touch screen): `Cx::hover_none`.
    const hoverNone = matchMedia('(hover: none)');
    const coarse = matchMedia('(pointer: coarse)');
    const pointer = () => callback(JSON.stringify({ kind: 'primary_pointer', hover: !hoverNone.matches, coarse: coarse.matches }));
    // A contrast theme (Windows' Contrast themes): `forced-colors`, and its system colours read
    // off a probe, as sRGB bytes, for `Cx::system_contrast`.
    const forced = matchMedia('(forced-colors: active)');
    const contrast = () => {
        if (!forced.matches) { callback(JSON.stringify({ kind: 'contrast', colors: null })); return; }
        const swatch = document.createElement('span');
        swatch.style.cssText = 'position:fixed;visibility:hidden;forced-color-adjust:none';
        document.body.append(swatch);
        const colors = ['Canvas', 'CanvasText', 'Highlight', 'HighlightText', 'GrayText', 'ButtonFace', 'ButtonText', 'LinkText'].map((name) => {
            swatch.style.color = name;
            return (getComputedStyle(swatch).color.match(/\d+(\.\d+)?/g) ?? ['0', '0', '0']).slice(0, 3).map(Number);
        });
        swatch.remove();
        callback(JSON.stringify({ kind: 'contrast', colors }));
    };
    const focus = () => callback(JSON.stringify({ kind: 'focus', focused: document.hasFocus() }));
    const languageChange = () => callback(JSON.stringify({ kind: 'languages', languages: preferredLanguages() }));
    const visibility = () => callback(JSON.stringify({ kind: 'visibility', visible: !document.hidden, focused: document.hasFocus() }));
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
    // The safe area can change with no resize (Safari settles nv() after a rotation's resize;
    // a Home Screen app's status bar): the probe's padding is the insets, so its border box follows
    // them (its content box is always empty, which is what an observer watches by default).
    const insets = new ResizeObserver(update);
    insets.observe(probe, { box: 'border-box' });
    motion.addEventListener('change', reduced);
    hoverNone.addEventListener('change', pointer);
    coarse.addEventListener('change', pointer);
    forced.addEventListener('change', contrast);
    window.addEventListener('focus', focus);
    window.addEventListener('blur', focus);
    window.addEventListener('languagechange', languageChange);
    document.addEventListener('visibilitychange', visibility);
    // Mount is called while Rust is installing the shell; enter it only after installation.
    queueMicrotask(() => { update(); reduced(); pointer(); contrast(); focus(); visibility(); });
    return () => {
        dprMedia?.removeEventListener('change', onDpr);
        window.visualViewport?.removeEventListener('resize', update);
        window.visualViewport?.removeEventListener('scroll', update);
        window.removeEventListener('resize', update);
        insets.disconnect();
        motion.removeEventListener('change', reduced);
        hoverNone.removeEventListener('change', pointer);
        coarse.removeEventListener('change', pointer);
        forced.removeEventListener('change', contrast);
        window.removeEventListener('focus', focus);
        window.removeEventListener('blur', focus);
        window.removeEventListener('languagechange', languageChange);
        document.removeEventListener('visibilitychange', visibility);
        probe.remove();
        largest.remove();
    };
}

// One face of an app font as the page's own, so the semantic mirror's text sets in it.
export function addFontFace(family, bytes, weight, style) {
    const face = new FontFace(family, bytes.slice(), { weight, style });
    document.fonts.add(face);
    face.load().catch(() => {});
}
