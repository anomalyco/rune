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

export function viewport(canvas, callback, embedded) {
    const probe = document.createElement('div');
    probe.style.cssText = 'position:fixed;visibility:hidden;pointer-events:none;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)';
    document.body.append(probe);
    // The largest viewport: on iOS Safari `100lvh` is the viewport with the toolbar collapsed, so
    // a canvas or band sized from it doesn't change as the toolbar comes and goes mid-scroll.
    const largest = document.createElement('div');
    largest.style.cssText = 'position:fixed;top:0;left:0;width:0;height:100lvh;visibility:hidden;pointer-events:none';
    document.body.append(largest);
    const update = () => {
        if (!embedded) {
            const v = window.visualViewport;
            const width = `${v?.width ?? innerWidth}px`;
            const height = `${v?.height ?? innerHeight}px`;
            const left = `${v?.offsetLeft ?? 0}px`;
            const top = `${v?.offsetTop ?? 0}px`;
            const sibling = canvas.previousElementSibling;
            const targets = sibling?.hasAttribute?.('data-rune-cpu') ? [canvas, sibling] : [canvas];
            for (const target of targets) {
                target.style.width = width;
                target.style.height = height;
                target.style.left = left;
                target.style.top = top;
            }
        }
        const s = getComputedStyle(probe);
        callback(JSON.stringify({ kind: 'viewport', h: largest.offsetHeight || innerHeight,
            insets: embedded ? [0, 0, 0, 0] : [s.paddingTop, s.paddingRight, s.paddingBottom, s.paddingLeft].map(parseFloat) }));
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
