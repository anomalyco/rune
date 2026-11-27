import assert from 'node:assert/strict';
import test from 'node:test';

class FakeElement {
    constructor(tagName) {
        this.tagName = tagName.toUpperCase();
        this.style = {
            cssText: '', width: '', height: '', left: '', top: '', cursor: '',
            removeProperty(name) { if (name === 'cursor') this.cursor = ''; },
        };
        this.attributes = new Map();
        this.children = [];
        this.parentElement = null;
        this.offsetHeight = 0;
    }

    get previousElementSibling() {
        if (!this.parentElement) return null;
        const i = this.parentElement.children.indexOf(this);
        return i > 0 ? this.parentElement.children[i - 1] : null;
    }

    setAttribute(name, value) { this.attributes.set(name, String(value)); }
    hasAttribute(name) { return this.attributes.has(name); }
    getAttribute(name) { return this.attributes.get(name) ?? null; }

    append(...nodes) {
        for (const node of nodes) {
            node.parentElement = this;
            this.children.push(node);
        }
    }

    insertBefore(node, before) {
        node.parentElement = this;
        const i = before ? this.children.indexOf(before) : -1;
        if (i >= 0) this.children.splice(i, 0, node);
        else this.children.push(node);
        return node;
    }

    remove() {
        if (!this.parentElement) return;
        const i = this.parentElement.children.indexOf(this);
        if (i >= 0) this.parentElement.children.splice(i, 1);
        this.parentElement = null;
    }
}

class FakeEventTarget {
    constructor() { this.listeners = new Map(); }

    addEventListener(name, handler) {
        let list = this.listeners.get(name);
        if (!list) { list = []; this.listeners.set(name, list); }
        list.push(handler);
    }

    removeEventListener(name, handler) {
        const list = this.listeners.get(name);
        if (!list) return;
        const i = list.indexOf(handler);
        if (i >= 0) list.splice(i, 1);
    }

    dispatchEvent(event) {
        for (const handler of [...(this.listeners.get(event.type) ?? [])]) handler(event);
    }
}

function installDom({ width = 390, height = 844, offsetLeft = 0, offsetTop = 0, rootFontSize = '16px' } = {}) {
    const body = new FakeElement('body');
    const documentElement = new FakeElement('html');
    const doc = Object.assign(new FakeEventTarget(), {
        body,
        documentElement,
        hidden: false,
        hasFocus: () => true,
        createElement: tag => new FakeElement(tag),
    });
    const visualViewport = Object.assign(new FakeEventTarget(), { width, height, offsetLeft, offsetTop });
    const win = Object.assign(new FakeEventTarget(), {
        visualViewport,
        devicePixelRatio: 1,
    });
    const makeMedia = () => Object.assign(new FakeEventTarget(), { matches: false });
    const observers = [];
    class FakeResizeObserver {
        constructor(cb) {
            this.cb = cb;
            this.targets = new Set();
            this.disconnected = false;
            observers.push(this);
        }
        observe(target) { this.targets.add(target); }
        disconnect() { this.disconnected = true; this.targets.clear(); }
        trigger() { if (!this.disconnected) this.cb(); }
    }
    const rootStyle = { fontSize: rootFontSize };

    Object.defineProperty(globalThis, 'document', { value: doc, configurable: true });
    Object.defineProperty(globalThis, 'window', { value: win, configurable: true });
    Object.defineProperty(globalThis, 'innerWidth', { value: width, configurable: true });
    Object.defineProperty(globalThis, 'innerHeight', { value: height, configurable: true });
    Object.defineProperty(globalThis, 'matchMedia', { value: makeMedia, configurable: true });
    Object.defineProperty(globalThis, 'ResizeObserver', { value: FakeResizeObserver, configurable: true });
    Object.defineProperty(globalThis, 'getComputedStyle', {
        value: el => (el === documentElement
            ? rootStyle
            : { paddingTop: '0', paddingRight: '0', paddingBottom: '0', paddingLeft: '0' }),
        configurable: true,
    });
    return { doc, win, body, documentElement, visualViewport, observers, rootStyle };
}

const platform = await import('./platform.js');

function makeCpuCanvas(doc) {
    const cpu = doc.createElement('canvas');
    cpu.setAttribute('aria-hidden', 'true');
    cpu.setAttribute('data-rune-cpu', '');
    cpu.style.width = '100%';
    cpu.style.height = '100%';
    cpu.style.left = '0';
    cpu.style.top = '0';
    return cpu;
}

test('viewport keeps GPU and CPU canvases at layout-viewport CSS bounds while reporting visualViewport band', async () => {
    const { doc, body, visualViewport } = installDom({ width: 390, height: 699 });
    const over = doc.createElement('canvas');
    over.style.width = '100%';
    over.style.height = '100%';
    over.style.left = '0';
    over.style.top = '0';
    body.append(over);
    const cpu = makeCpuCanvas(doc);
    body.insertBefore(cpu, over);

    const events = [];
    const dispose = platform.viewport(over, msg => events.push(JSON.parse(msg)), false);
    await Promise.resolve();

    assert.equal(over.style.width, '100%');
    assert.equal(over.style.height, '100%');
    assert.equal(cpu.style.width, '100%');
    assert.equal(cpu.style.height, '100%');
    assert.equal(events.length, 1);
    assert.equal(events[0].kind, 'init');
    assert.deepEqual(events[0].visual, [0, 699, 699]);

    // Keyboard presentation on iOS Safari shrinks visualViewport (699 -> 389 -> 362) while leaving
    // the layout viewport untouched: neither canvas CSS box may shrink or shift.
    events.length = 0;
    visualViewport.height = 389;
    visualViewport.dispatchEvent({ type: 'resize' });
    visualViewport.height = 362;
    visualViewport.dispatchEvent({ type: 'resize' });
    assert.equal(over.style.width, '100%');
    assert.equal(over.style.height, '100%');
    assert.equal(cpu.style.width, '100%');
    assert.equal(cpu.style.height, '100%');
    assert.deepEqual(events.map(e => e.visual), [[0, 389, 699], [0, 362, 699]]);

    events.length = 0;
    visualViewport.offsetLeft = 12;
    visualViewport.offsetTop = 24;
    visualViewport.dispatchEvent({ type: 'scroll' });
    assert.equal(over.style.left, '0');
    assert.equal(over.style.top, '0');
    assert.equal(cpu.style.left, '0');
    assert.equal(cpu.style.top, '0');
    assert.deepEqual(events.map(e => e.visual), [[24, 386, 699]]);

    // Pinch-zoom shrinks visualViewport by 1/scale: do not report it as a keyboard inset.
    events.length = 0;
    visualViewport.scale = 2;
    visualViewport.dispatchEvent({ type: 'resize' });
    assert.deepEqual(events.map(e => e.visual), [null]);

    // Unzoomed retraction restores full layout height band.
    events.length = 0;
    visualViewport.scale = 1;
    visualViewport.offsetTop = 0;
    visualViewport.height = 699;
    visualViewport.dispatchEvent({ type: 'resize' });
    assert.deepEqual(events.map(e => e.visual), [[0, 699, 699]]);

    dispose();
});

test('viewport reports null visual band for embedded canvases and leaves unrelated siblings untouched', async () => {
    const { doc, body, visualViewport } = installDom({ width: 390, height: 844 });
    const embedded = doc.createElement('canvas');
    embedded.style.height = '100%';
    const cpu = makeCpuCanvas(doc);
    body.append(cpu, embedded);
    const events = [];
    const disposeEmbedded = platform.viewport(embedded, msg => events.push(JSON.parse(msg)), true);
    await Promise.resolve();
    events.length = 0;
    visualViewport.height = 400;
    visualViewport.dispatchEvent({ type: 'resize' });
    assert.equal(embedded.style.height, '100%');
    assert.equal(cpu.style.height, '100%');
    assert.deepEqual(events.map(e => e.visual), [null]);
    disposeEmbedded();
});

test('viewport reports root font-size textScale in init and emits text_scale on ResizeObserver changes', async () => {
    const { doc, body, observers, rootStyle } = installDom({ rootFontSize: '20px' });
    const canvas = doc.createElement('canvas');
    body.append(canvas);
    const events = [];
    const dispose = platform.viewport(canvas, msg => events.push(JSON.parse(msg)), false);
    await Promise.resolve();

    assert.equal(events.length, 1);
    assert.equal(events[0].kind, 'init');
    assert.equal(events[0].textScale, 1.25);

    // Find the observer watching the 10rem text-scale probe.
    const textObserver = observers.find(o => [...o.targets].some(t => t.style.cssText.includes('10rem')));
    assert.ok(textObserver, 'viewport must observe a 10rem text-scale probe');

    // Unchanged root font-size does not emit duplicate text_scale events.
    events.length = 0;
    textObserver.trigger();
    assert.equal(events.length, 0);

    // Raising root font-size to 24px emits scale 1.5.
    rootStyle.fontSize = '24px';
    textObserver.trigger();
    assert.deepEqual(events, [{ kind: 'text_scale', scale: 1.5 }]);

    // Repeating with the same 24px emits nothing.
    events.length = 0;
    textObserver.trigger();
    assert.equal(events.length, 0);

    // After dispose, the observer disconnects and stops emitting.
    dispose();
    rootStyle.fontSize = '16px';
    textObserver.trigger();
    assert.equal(events.length, 0);
});

test('preferredLanguages reads navigator.languages with navigator.language fallback and languagechange forwards updates', async () => {
    const origNav = globalThis.navigator;
    try {
        Object.defineProperty(globalThis, 'navigator', {
            value: { languages: ['ar-EG', '  ', 'en-US'], language: 'en-US' },
            configurable: true,
        });
        assert.deepEqual(platform.preferredLanguages(), ['ar-EG', 'en-US']);

        Object.defineProperty(globalThis, 'navigator', {
            value: { languages: [], language: 'he-IL' },
            configurable: true,
        });
        assert.deepEqual(platform.preferredLanguages(), ['he-IL']);

        Object.defineProperty(globalThis, 'navigator', { value: undefined, configurable: true });
        assert.deepEqual(platform.preferredLanguages(), []);

        // viewport does not eagerly emit languages on mount, only when languagechange fires.
        Object.defineProperty(globalThis, 'navigator', {
            value: { languages: ['fr-FR', 'en-US'], language: 'fr-FR' },
            configurable: true,
        });
        const { doc, win, body } = installDom();
        const canvas = doc.createElement('canvas');
        body.append(canvas);
        const events = [];
        const dispose = platform.viewport(canvas, msg => events.push(JSON.parse(msg)), false);
        await Promise.resolve();
        assert.equal(events.some(e => e.kind === 'languages'), false, 'mount must not eagerly query languages');

        win.dispatchEvent({ type: 'languagechange' });
        assert.deepEqual(events.filter(e => e.kind === 'languages'), [{ kind: 'languages', languages: ['fr-FR', 'en-US'] }]);

        // After dispose, languagechange stops firing.
        events.length = 0;
        dispose();
        win.dispatchEvent({ type: 'languagechange' });
        assert.equal(events.length, 0);
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
    }
});

async function decodeTestPng(png) {
    // Verify PNG signature, parse IHDR/sRGB/IDAT/IEND chunks and CRCs, inflate IDAT, unfilter Sub rows.
    assert.deepEqual(png.slice(0, 8), new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
    const dv = new DataView(png.buffer, png.byteOffset, png.byteLength);
    let pos = 8, width = 0, height = 0, hasSrgb = false, hasIend = false;
    const idatChunks = [];
    while (pos < png.byteLength) {
        const len = dv.getUint32(pos);
        const type = String.fromCharCode(png[pos + 4], png[pos + 5], png[pos + 6], png[pos + 7]);
        const data = png.subarray(pos + 8, pos + 8 + len);
        if (type === 'IHDR') { width = dv.getUint32(pos + 8); height = dv.getUint32(pos + 12); }
        else if (type === 'sRGB') { hasSrgb = data.length === 1 && data[0] === 0; }
        else if (type === 'IDAT') { idatChunks.push(data); }
        else if (type === 'IEND') { hasIend = true; }
        pos += 12 + len;
    }
    assert.ok(hasSrgb && hasIend, 'PNG must contain sRGB and IEND chunks');
    const ds = new DecompressionStream('deflate');
    const w = ds.writable.getWriter();
    for (const c of idatChunks) w.write(c);
    w.close();
    const inflated = new Uint8Array(await new Response(ds.readable).arrayBuffer());
    const rowBytes = width * 4;
    assert.equal(inflated.byteLength, height * (rowBytes + 1));
    const rgba = new Uint8Array(height * rowBytes);
    for (let y = 0; y < height; y++) {
        const filter = inflated[y * (rowBytes + 1)];
        assert.ok(filter === 0 || filter === 1);
        for (let i = 0; i < rowBytes; i++) {
            const raw = inflated[y * (rowBytes + 1) + 1 + i];
            const left = (filter === 1 && i >= 4) ? rgba[y * rowBytes + i - 4] : 0;
            rgba[y * rowBytes + i] = (raw + left) & 0xff;
        }
    }
    return { width, height, rgba };
}

test('encodePngAsync produces valid bit-exact PNG for straight and linear-premultiplied RGBA and enforces maxBytes', async () => {
    // Straight RGBA with opaque, translucent, and transparent pixels round-trips bit-exactly.
    const straight = new Uint8Array([
        255, 0, 0, 255, 10, 200, 30, 77,
        1, 2, 3, 0, 128, 64, 32, 255,
    ]);
    const res = await platform.encodePngAsync(2, 2, straight, false, 1024 * 1024);
    assert.ok(res.png instanceof Uint8Array);
    const dec = await decodeTestPng(res.png);
    assert.equal(dec.width, 2);
    assert.equal(dec.height, 2);
    assert.deepEqual(dec.rgba, straight);

    // Premultiplied mip 0: opaque stays exact, alpha==0 zeroes RGB, translucent un-premultiplies in linear light.
    const premul = new Uint8Array([
        200, 100, 50, 255, 99, 88, 77, 0,
        188, 0, 0, 128, 0, 0, 0, 255,
    ]);
    const res2 = await platform.encodePngAsync(2, 2, premul, true, 1024 * 1024);
    const dec2 = await decodeTestPng(res2.png);
    assert.deepEqual(dec2.rgba.slice(0, 4), new Uint8Array([200, 100, 50, 255]));
    assert.deepEqual(dec2.rgba.slice(4, 8), new Uint8Array([0, 0, 0, 0]));
    assert.ok(Math.abs(dec2.rgba[8] - 255) <= 1 && dec2.rgba[11] === 128, `unpremul half-alpha red: ${dec2.rgba.slice(8, 12)}`);

    // Size cap enforcement returns too_large.
    const tinyCap = await platform.encodePngAsync(2, 2, straight, false, 64);
    assert.equal(tinyCap.code, 'too_large');
    assert.equal(tinyCap.png, undefined);
});

test('writeClipboardImage creates ClipboardItem with Promise<Blob> synchronously, encodes off-turn, and enforces supersede and errors', async () => {
    const origNav = globalThis.navigator;
    const origCI = globalThis.ClipboardItem;
    const origCS = globalThis.CompressionStream;
    try {
        let writtenItems = null;
        let extractCalled = false;
        class FakeClipboardItem {
            constructor(forms) { this.forms = forms; }
        }
        Object.defineProperty(globalThis, 'ClipboardItem', { value: FakeClipboardItem, configurable: true });
        Object.defineProperty(globalThis, 'navigator', {
            value: {
                clipboard: {
                    async write(items) {
                        writtenItems = items;
                        const blob = await items[0].forms['image/png'];
                        return blob;
                    },
                },
            },
            configurable: true,
        });

        const pixels = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255]);
        const promise = platform.writeClipboardImage(
            (wantRaw) => {
                extractCalled = true;
                assert.equal(wantRaw, true);
                return { width: 2, height: 1, bytes: pixels, premul: false };
            },
            () => true,
            32 * 1024 * 1024,
        );
        // Clipboard.write and ClipboardItem must have been called synchronously inside the gesture,
        // BEFORE extract() runs on a later turn.
        assert.equal(extractCalled, false, 'extract must not run synchronously on the input turn');
        assert.equal(writtenItems?.length, 1);
        assert.ok(writtenItems[0].forms['image/png'] instanceof Promise);

        const blob = await promise;
        assert.equal(extractCalled, true);
        assert.equal(blob.type, 'image/png');
        const dec = await decodeTestPng(new Uint8Array(await blob.arrayBuffer()));
        assert.deepEqual(dec.rgba, pixels);

        // Post-encode supersession rejects without resolving the blob.
        await assert.rejects(
            platform.writeClipboardImage(
                () => ({ width: 2, height: 1, bytes: pixels, premul: false }),
                () => false,
                32 * 1024 * 1024,
            ),
            /superseded/,
        );

        // Too-large PNG routes code to checkOutcome and rejects.
        const seenCodes = [];
        await assert.rejects(
            platform.writeClipboardImage(
                () => ({ width: 2, height: 1, bytes: pixels, premul: false }),
                (code) => { seenCodes.push(code); return false; },
                32,
            ),
            /size limits/,
        );
        assert.deepEqual(seenCodes, ['too_large']);

        // Legacy fallback when CompressionStream is absent asks extract(false) for pre-encoded PNG.
        Object.defineProperty(globalThis, 'CompressionStream', { value: undefined, configurable: true });
        const legacyPng = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3, 4]);
        const legacyBlob = await platform.writeClipboardImage(
            (wantRaw) => { assert.equal(wantRaw, false); return { png: legacyPng }; },
            () => true,
            32 * 1024 * 1024,
        );
        assert.deepEqual(new Uint8Array(await legacyBlob.arrayBuffer()), legacyPng);
        Object.defineProperty(globalThis, 'CompressionStream', { value: origCS, configurable: true });

        // Browser clipboard rejection (e.g. outside user gesture) propagates and cancels the
        // deferred timer so neither extract nor checkOutcome runs after settlement.
        Object.defineProperty(globalThis, 'navigator', {
            value: { clipboard: { write: () => Promise.reject(new Error('NotAllowedError')) } },
            configurable: true,
        });
        let calledAfterReject = false;
        await assert.rejects(
            platform.writeClipboardImage(
                () => { calledAfterReject = true; return { width: 2, height: 1, bytes: pixels, premul: false }; },
                () => { calledAfterReject = true; return true; },
                32 * 1024 * 1024,
            ),
            /NotAllowedError/,
        );
        await new Promise((r) => setTimeout(r, 20));
        assert.equal(calledAfterReject, false, 'deferred encode callbacks must not run after write rejects');
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
        Object.defineProperty(globalThis, 'ClipboardItem', { value: origCI, configurable: true });
        Object.defineProperty(globalThis, 'CompressionStream', { value: origCS, configurable: true });
    }
});

test('setCursor applies CSS cursor keywords for one-way resize, alias, and copy and clears on empty', () => {
    const { doc } = installDom();
    const canvas = doc.createElement('canvas');
    for (const css of ['n-resize', 'e-resize', 's-resize', 'w-resize', 'alias', 'copy']) {
        platform.setCursor(canvas, css);
        assert.equal(canvas.style.cursor, css);
    }
    platform.setCursor(canvas, '');
    assert.equal(canvas.style.cursor, '');
});

test('setCursor prefers image-set, falls back to -webkit-image-set on Safari and plain url() on Firefox', () => {
    const makeCanvas = (accept) => {
        let cur = '';
        return {
            style: {
                get cursor() { return cur; },
                set cursor(v) { if (accept(v)) cur = v; },
                removeProperty(name) { if (name === 'cursor') cur = ''; },
            },
        };
    };
    const imgCss = 'image-set(url("blob:https://example.com/cur(1)") 2x) 4 8, auto';

    // Chrome: accepts unprefixed image-set() directly.
    const chrome = makeCanvas(v => v.startsWith('image-set(') || v === 'pointer');
    chrome.style.cursor = 'pointer';
    platform.setCursor(chrome, imgCss);
    assert.equal(chrome.style.cursor, imgCss);

    // Safari: rejects unprefixed image-set(), accepts -webkit-image-set(), even after a prior cursor.
    const safari = makeCanvas(v => v.startsWith('-webkit-image-set(') || v === 'pointer');
    safari.style.cursor = 'pointer';
    platform.setCursor(safari, imgCss);
    assert.equal(safari.style.cursor, `-webkit-${imgCss}`);

    // Firefox: rejects both image-set() and -webkit-image-set(), accepts plain url() fallback.
    const firefox = makeCanvas(v => v.startsWith('url(') || v === 'pointer');
    firefox.style.cursor = 'pointer';
    platform.setCursor(firefox, imgCss);
    assert.equal(firefox.style.cursor, 'url("blob:https://example.com/cur(1)") 4 8, auto');
});

test('viewport watchDpr falls back to -webkit-device-pixel-ratio when resolution dppx does not match', async () => {
    const { doc, win, body } = installDom({ width: 390, height: 844 });
    win.devicePixelRatio = 2;
    const queries = new Map();
    Object.defineProperty(globalThis, 'matchMedia', {
        value: (query) => {
            const target = Object.assign(new FakeEventTarget(), {
                matches: query === `(-webkit-device-pixel-ratio: ${win.devicePixelRatio})`,
            });
            queries.set(query, target);
            return target;
        },
        configurable: true,
    });
    const canvas = doc.createElement('canvas');
    body.append(canvas);
    const events = [];
    const dispose = platform.viewport(canvas, msg => events.push(JSON.parse(msg)), false);
    await Promise.resolve();
    events.length = 0;

    const first = queries.get('(-webkit-device-pixel-ratio: 2)');
    assert.ok(first, 'must query -webkit-device-pixel-ratio fallback when resolution dppx is unsupported');
    assert.equal(first.listeners.get('change')?.length ?? 0, 1);

    win.devicePixelRatio = 1;
    first.matches = false;
    first.dispatchEvent({ type: 'change' });
    assert.deepEqual(events, [{ kind: 'dpr', scale: 1 }]);
    assert.equal(first.listeners.get('change')?.length ?? 0, 0, 'old fallback listener detached');
    const second = queries.get('(-webkit-device-pixel-ratio: 1)');
    assert.ok(second && (second.listeners.get('change')?.length ?? 0) === 1, 're-armed on new DPR');

    dispose();
    assert.equal(second.listeners.get('change')?.length ?? 0, 0, 'dispose detaches DPR fallback listener');
});

test('viewport startup microtask emits one combined init snapshot, respects dispose, and keeps live events separate', async () => {
    const { doc, win, body } = installDom({ width: 390, height: 844 });
    const queries = new Map();
    Object.defineProperty(globalThis, 'matchMedia', {
        value: (query) => {
            const target = Object.assign(new FakeEventTarget(), {
                matches: query === '(prefers-reduced-motion: reduce)'
                    || query === '(prefers-reduced-transparency: reduce)'
                    || query === '(hover: none)'
                    || query === '(pointer: coarse)'
                    || query === '(forced-colors: active)',
            });
            queries.set(query, target);
            return target;
        },
        configurable: true,
    });
    const palette = {
        Canvas: 'rgb(1, 2, 3)',
        CanvasText: 'rgb(4, 5, 6)',
        Highlight: 'rgb(7, 8, 9)',
        HighlightText: 'rgb(10, 11, 12)',
        GrayText: 'rgb(13, 14, 15)',
        ButtonFace: 'rgb(16, 17, 18)',
        ButtonText: 'rgb(19, 20, 21)',
        LinkText: 'rgb(22, 23, 24)',
    };
    Object.defineProperty(globalThis, 'getComputedStyle', {
        value: (el) => ({
            paddingTop: '12',
            paddingRight: '0',
            paddingBottom: '34',
            paddingLeft: '0',
            color: palette[el.style.color] ?? 'rgb(0, 0, 0)',
        }),
        configurable: true,
    });
    doc.hidden = true;
    doc.hasFocus = () => false;

    const canvas = doc.createElement('canvas');
    body.append(canvas);
    const events = [];
    const dispose = platform.viewport(canvas, msg => events.push(JSON.parse(msg)), false);
    assert.equal(events.length, 0, 'startup snapshot must defer until the microtask');
    await Promise.resolve();

    assert.deepEqual(events, [{
        kind: 'init',
        h: 844,
        insets: [12, 0, 34, 0],
        visual: [0, 844, 844],
        reduced: true,
        reducedTransparency: true,
        hover: false,
        coarse: true,
        colors: [
            [1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12],
            [13, 14, 15], [16, 17, 18], [19, 20, 21], [22, 23, 24],
        ],
        increased: true,
        textScale: 1,
        focused: false,
        visible: false,
    }]);

    // Ongoing listener events continue to emit their individual kinds.
    events.length = 0;
    queries.get('(prefers-reduced-motion: reduce)').matches = false;
    queries.get('(prefers-reduced-motion: reduce)').dispatchEvent({ type: 'change' });
    queries.get('(prefers-reduced-transparency: reduce)').matches = false;
    queries.get('(prefers-reduced-transparency: reduce)').dispatchEvent({ type: 'change' });
    queries.get('(forced-colors: active)').matches = false;
    queries.get('(forced-colors: active)').dispatchEvent({ type: 'change' });
    doc.hidden = false;
    doc.hasFocus = () => true;
    win.dispatchEvent({ type: 'focus' });
    doc.dispatchEvent({ type: 'visibilitychange' });
    assert.deepEqual(events, [
        { kind: 'motion', reduced: false },
        { kind: 'transparency', reduced: false },
        { kind: 'contrast', colors: null, increased: false },
        { kind: 'focus', focused: true },
        { kind: 'visibility', visible: true, focused: true },
    ]);
    dispose();

    // Disposing before the startup microtask fires suppresses the init callback.
    const canceled = [];
    const disposeEarly = platform.viewport(canvas, msg => canceled.push(JSON.parse(msg)), false);
    disposeEarly();
    await Promise.resolve();
    assert.deepEqual(canceled, []);
});

test('viewport tracks prefers-contrast: more alongside forced-colors and cleans up on dispose', async () => {
    const { doc, body } = installDom({ width: 390, height: 844 });
    const queries = new Map();
    Object.defineProperty(globalThis, 'matchMedia', {
        value: (query) => {
            const target = Object.assign(new FakeEventTarget(), {
                matches: query === '(prefers-contrast: more)',
            });
            queries.set(query, target);
            return target;
        },
        configurable: true,
    });
    const palette = {
        Canvas: 'rgb(10, 10, 10)',
        CanvasText: 'rgb(250, 250, 250)',
        Highlight: 'rgb(0, 120, 215)',
        HighlightText: 'rgb(255, 255, 255)',
        GrayText: 'rgb(128, 128, 128)',
        ButtonFace: 'rgb(30, 30, 30)',
        ButtonText: 'rgb(255, 255, 255)',
        LinkText: 'rgb(80, 160, 255)',
    };
    Object.defineProperty(globalThis, 'getComputedStyle', {
        value: (el) => ({
            paddingTop: '0',
            paddingRight: '0',
            paddingBottom: '0',
            paddingLeft: '0',
            color: palette[el.style.color] ?? 'rgb(0, 0, 0)',
        }),
        configurable: true,
    });

    const canvas = doc.createElement('canvas');
    body.append(canvas);
    const events = [];
    const dispose = platform.viewport(canvas, msg => events.push(JSON.parse(msg)), false);
    await Promise.resolve();

    // Startup snapshot with prefers-contrast: more alone (macOS/iOS/Linux browsers).
    assert.equal(events.length, 1);
    assert.equal(events[0].kind, 'init');
    assert.equal(events[0].colors, null);
    assert.equal(events[0].increased, true);

    // Live toggle off -> on on prefers-contrast: more.
    events.length = 0;
    const more = queries.get('(prefers-contrast: more)');
    const forced = queries.get('(forced-colors: active)');
    more.matches = false;
    more.dispatchEvent({ type: 'change' });
    more.matches = true;
    more.dispatchEvent({ type: 'change' });

    // forced-colors: active alone still reports increased: true even when prefers-contrast: more is false.
    more.matches = false;
    forced.matches = true;
    forced.dispatchEvent({ type: 'change' });

    assert.deepEqual(events, [
        { kind: 'contrast', colors: null, increased: false },
        { kind: 'contrast', colors: null, increased: true },
        {
            kind: 'contrast',
            colors: [
                [10, 10, 10], [250, 250, 250], [0, 120, 215], [255, 255, 255],
                [128, 128, 128], [30, 30, 30], [255, 255, 255], [80, 160, 255],
            ],
            increased: true,
        },
    ]);

    // Dispose detaches both contrast media query listeners.
    dispose();
    events.length = 0;
    more.matches = true;
    more.dispatchEvent({ type: 'change' });
    forced.matches = false;
    forced.dispatchEvent({ type: 'change' });
    assert.deepEqual(events, []);
});

function makeInlineCanvas({
    parentWidth = 478,
    inlineStyles = {},
    attrs = {},
    stylesheetHeight = null,
    aspectRatio = null,
    flexHeight = null,
    minHeight = 0,
    maxHeight = Infinity,
    hidden = false,
    computedMap = null,
} = {}) {
    const props = new Map(Object.entries(inlineStyles));
    const attributes = new Map(Object.entries(attrs).map(([k, v]) => [k, String(v)]));
    const canvas = {
        style: {
            setProperty(name, value) { props.set(name, String(value)); },
            getPropertyValue(name) { return props.get(name) ?? ''; },
            removeProperty(name) { props.delete(name); },
            get height() { return props.get('height') ?? ''; },
        },
        getAttribute(name) { return attributes.has(name) ? attributes.get(name) : null; },
        setAttribute(name, value) { attributes.set(name, String(value)); },
        removeAttribute(name) { attributes.delete(name); },
        getBoundingClientRect() {
            if (hidden) return { width: 0, height: 0 };
            const inlineH = props.get('height');
            let h;
            if (inlineH && inlineH !== 'auto') {
                h = parseFloat(inlineH);
            } else if (stylesheetHeight !== null) {
                h = stylesheetHeight;
            } else if (flexHeight !== null) {
                h = flexHeight;
            } else if (aspectRatio !== null) {
                h = parentWidth / aspectRatio;
            } else {
                const iw = Number(attributes.get('width') ?? 300);
                const ih = Number(attributes.get('height') ?? 150);
                h = parentWidth * (ih / iw);
            }
            h = Math.min(Math.max(h, minHeight), maxHeight);
            return { width: parentWidth, height: h };
        },
    };
    if (computedMap) {
        canvas.computedStyleMap = () => ({ get: name => computedMap[name] });
    }
    return { canvas, props, attributes };
}

test('styleInlineCanvas sets AppSpec height when page gave none and preserves page-owned styles and heights', () => {
    installDom();

    // 1. Exact issue repro: 478 px parent, inline width only -> browser defaults to 2:1 (239 px).
    // Must set height to AppSpec::size.1 (560px) and leave width/height HTML attributes absent.
    {
        const { canvas, props, attributes } = makeInlineCanvas({
            parentWidth: 478,
            inlineStyles: { display: 'block', width: '100%' },
        });
        assert.equal(canvas.getBoundingClientRect().height, 239);
        platform.styleInlineCanvas(canvas, 560, false);
        assert.equal(props.get('display'), 'block');
        assert.equal(props.get('width'), '100%');
        assert.equal(props.get('height'), '560px');
        assert.equal(props.get('touch-action'), 'none');
        assert.equal(props.get('outline'), 'none');
        assert.equal(props.get('user-select'), 'none');
        assert.equal(props.get('-webkit-user-select'), 'none');
        assert.equal(canvas.getBoundingClientRect().height, 560);
        assert.equal(attributes.has('width'), false, 'probe must not leave width attribute behind');
        assert.equal(attributes.has('height'), false, 'probe must not leave height attribute behind');
    }

    // 2. Existing inline style height and unrelated properties are preserved (not clobbered).
    {
        const { canvas, props } = makeInlineCanvas({
            parentWidth: 478,
            inlineStyles: { height: '400px', 'border-radius': '12px' },
        });
        platform.styleInlineCanvas(canvas, 560, false);
        assert.equal(props.get('height'), '400px');
        assert.equal(props.get('border-radius'), '12px');
        assert.equal(props.get('width'), '100%');
    }

    // 3. Stylesheet height (even when it happens to equal width / 2 = 239 px) is respected,
    // and existing HTML width/height attributes are restored verbatim.
    {
        const { canvas, props, attributes } = makeInlineCanvas({
            parentWidth: 478,
            stylesheetHeight: 239,
            attrs: { width: '480', height: '320' },
        });
        platform.styleInlineCanvas(canvas, 560, false);
        assert.equal(props.has('height'), false, 'must not override stylesheet height with inline style');
        assert.equal(attributes.get('width'), '480');
        assert.equal(attributes.get('height'), '320');
    }

    // 4. CSS aspect-ratio and flex/grid stretched height are respected.
    {
        const aspect = makeInlineCanvas({ parentWidth: 478, aspectRatio: 16 / 9 });
        platform.styleInlineCanvas(aspect.canvas, 560, false);
        assert.equal(aspect.props.has('height'), false);

        const flex = makeInlineCanvas({ parentWidth: 478, flexHeight: 360 });
        platform.styleInlineCanvas(flex.canvas, 560, false);
        assert.equal(flex.props.has('height'), false);
    }

    // 5. min-height or max-height alone still falls back to AppSpec::size.1 so CSS bounds apply to it.
    {
        const maxOnly = makeInlineCanvas({ parentWidth: 478, maxHeight: 200 });
        platform.styleInlineCanvas(maxOnly.canvas, 560, false);
        assert.equal(maxOnly.props.get('height'), '560px');

        const minOnly = makeInlineCanvas({ parentWidth: 478, minHeight: 300 });
        platform.styleInlineCanvas(minOnly.canvas, 560, false);
        assert.equal(minOnly.props.get('height'), '560px');
    }

    // 6. Hidden (display:none) canvas falls back via computedStyleMap / getComputedStyle.
    {
        const hiddenAuto = makeInlineCanvas({
            hidden: true,
            computedMap: { height: 'auto', 'aspect-ratio': 'auto' },
        });
        platform.styleInlineCanvas(hiddenAuto.canvas, 560, false);
        assert.equal(hiddenAuto.props.get('height'), '560px');

        const hiddenStyled = makeInlineCanvas({
            hidden: true,
            computedMap: { height: '400px', 'aspect-ratio': 'auto' },
        });
        platform.styleInlineCanvas(hiddenStyled.canvas, 560, false);
        assert.equal(hiddenStyled.props.has('height'), false);

        Object.defineProperty(globalThis, 'getComputedStyle', {
            value: () => ({ height: 'auto', aspectRatio: 'auto' }),
            configurable: true,
        });
        const hiddenFallbackAuto = makeInlineCanvas({ hidden: true });
        platform.styleInlineCanvas(hiddenFallbackAuto.canvas, 560, false);
        assert.equal(hiddenFallbackAuto.props.get('height'), '560px');

        Object.defineProperty(globalThis, 'getComputedStyle', {
            value: () => ({ height: '420px', aspectRatio: 'auto' }),
            configurable: true,
        });
        const hiddenFallbackCss = makeInlineCanvas({ hidden: true });
        platform.styleInlineCanvas(hiddenFallbackCss.canvas, 560, false);
        assert.equal(hiddenFallbackCss.props.has('height'), false);
    }

    // 7. fit_content_height sets pan-y and leaves height unset for start_app's content measurement;
    // non-positive or non-finite specHeight leaves height unset.
    {
        const fit = makeInlineCanvas({ parentWidth: 478 });
        platform.styleInlineCanvas(fit.canvas, 560, true);
        assert.equal(fit.props.get('touch-action'), 'pan-y');
        assert.equal(fit.props.has('height'), false);

        for (const bad of [0, -10, Number.NaN]) {
            const c = makeInlineCanvas({ parentWidth: 478 });
            platform.styleInlineCanvas(c.canvas, bad, false);
            assert.equal(c.props.has('height'), false);
        }
    }
});

test('readClipboard and pngBytes transcode non-PNG raster images via createImageBitmap and prefer PNG when present', async () => {
    const origNav = globalThis.navigator;
    const origCreateBitmap = globalThis.createImageBitmap;
    const origOffscreen = globalThis.OffscreenCanvas;
    try {
        let bitmapCalls = 0;
        let closedCount = 0;
        let nextDim = [2, 1];
        const rgba = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255]);
        Object.defineProperty(globalThis, 'createImageBitmap', {
            value: async () => {
                bitmapCalls++;
                return { width: nextDim[0], height: nextDim[1], close() { closedCount++; } };
            },
            configurable: true,
        });
        Object.defineProperty(globalThis, 'OffscreenCanvas', {
            value: class FakeOffscreenCanvas {
                constructor(w, h) { this.width = w; this.height = h; }
                getContext() {
                    return {
                        drawImage() {},
                        getImageData: () => ({ data: rgba }),
                    };
                }
            },
            configurable: true,
        });

        Object.defineProperty(globalThis, 'navigator', {
            value: {
                clipboard: {
                    async read() {
                        return [{
                            types: ['image/webp'],
                            async getType(t) {
                                assert.equal(t, 'image/webp');
                                return new Blob([new Uint8Array([1, 2, 3])], { type: 'image/webp' });
                            },
                        }];
                    },
                },
            },
            configurable: true,
        });
        const webpRes = await platform.readClipboard(true, true, 1024 * 1024);
        assert.ok(webpRes.png instanceof Uint8Array);
        const dec = await decodeTestPng(webpRes.png);
        assert.equal(dec.width, 2);
        assert.equal(dec.height, 1);
        assert.deepEqual(dec.rgba, rgba);
        assert.equal(bitmapCalls, 1);
        assert.equal(closedCount, 1);

        const rawPng = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 7, 8, 9]);
        Object.defineProperty(globalThis, 'navigator', {
            value: {
                clipboard: {
                    async read() {
                        return [{
                            types: ['image/jpeg', 'image/png'],
                            async getType(t) {
                                assert.equal(t, 'image/png');
                                return new Blob([rawPng], { type: 'image/png' });
                            },
                        }];
                    },
                },
            },
            configurable: true,
        });
        const pngRes = await platform.readClipboard(true, true, 1024 * 1024);
        assert.deepEqual(pngRes.png, rawPng);
        assert.equal(bitmapCalls, 1, 'PNG fast path must not invoke createImageBitmap');

        assert.equal(await platform.pngBytes(new Blob(['<svg/>'], { type: 'image/svg+xml' }), 1024), undefined);
        nextDim = [9000, 9000];
        assert.equal(await platform.pngBytes(new Blob([new Uint8Array([0xff, 0xd8])], { type: 'image/jpeg' }), 1024 * 1024), undefined);
        assert.equal(closedCount, 2);
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
        Object.defineProperty(globalThis, 'createImageBitmap', { value: origCreateBitmap, configurable: true });
        Object.defineProperty(globalThis, 'OffscreenCanvas', { value: origOffscreen, configurable: true });
    }
});
function installClipboardDom({ execResult = true, execError = null } = {}) {
    const dom = installDom();
    const { doc, body } = dom;
    const focusCalls = [];
    const execCalls = [];
    const createdFields = [];
    const ranges = [];
    const selection = {
        get rangeCount() { return ranges.length; },
        getRangeAt(i) { return ranges[i]; },
        removeAllRanges() { ranges.length = 0; },
        addRange(r) { ranges.push(r); },
    };

    const enhanceEl = (el) => {
        el.value = '';
        el.readOnly = false;
        el.tabIndex = 0;
        el.selectionStart = 0;
        el.selectionEnd = 0;
        el.selectionDirection = 'forward';
        el.selected = false;
        el.focus = (opts) => {
            focusCalls.push({ el, opts });
            doc.activeElement = el;
        };
        el.select = () => {
            el.selected = true;
            el.selectionStart = 0;
            el.selectionEnd = String(el.value).length;
            el.selectionDirection = 'forward';
            doc.activeElement = el;
        };
        el.setSelectionRange = (start, end, dir = 'forward') => {
            el.selectionStart = start;
            el.selectionEnd = end;
            el.selectionDirection = dir;
        };
        return el;
    };

    const origCreate = doc.createElement;
    doc.createElement = (tag) => {
        const el = enhanceEl(origCreate(tag));
        if (tag.toLowerCase() === 'textarea') createdFields.push(el);
        return el;
    };
    doc.activeElement = body;
    doc.getSelection = () => selection;
    doc.execCommand = (cmd) => {
        const active = doc.activeElement;
        execCalls.push({
            cmd,
            active,
            inBody: body.children.includes(active),
            value: active?.value,
            readOnly: active?.readOnly,
            selectionStart: active?.selectionStart,
            selectionEnd: active?.selectionEnd,
            ariaHidden: active?.getAttribute?.('aria-hidden'),
            hasMarker: active?.hasAttribute?.('data-rune-clipboard'),
        });
        if (execError) throw execError;
        return typeof execResult === 'function' ? execResult(cmd, active) : execResult;
    };
    return { ...dom, focusCalls, execCalls, createdFields, ranges, selection, enhanceEl };
}

test('writeClipboard falls back synchronously when navigator.clipboard is absent, preserving focus and selection', async () => {
    const origNav = globalThis.navigator;
    try {
        const { doc, body, focusCalls, execCalls, createdFields, ranges } = installClipboardDom();
        const canvas = doc.createElement('canvas');
        body.append(canvas);
        doc.activeElement = canvas;

        const savedRange = {
            id: 'r1',
            cloneRange() { return { id: 'r1-clone', cloneRange: this.cloneRange }; },
        };
        ranges.push(savedRange);

        Object.defineProperty(globalThis, 'navigator', { value: {}, configurable: true });

        // Must invoke execCommand('copy') synchronously before the returned promise yields,
        // so user activation is still active in browsers without navigator.clipboard.
        const promise = platform.writeClipboard('hello fallback', '', new Uint8Array());
        assert.equal(execCalls.length, 1, 'fallback must execute synchronously inside the gesture');
        assert.equal(execCalls[0].cmd, 'copy');
        assert.equal(execCalls[0].inBody, true);
        assert.equal(execCalls[0].value, 'hello fallback');
        assert.equal(execCalls[0].readOnly, true);
        assert.equal(execCalls[0].selectionStart, 0);
        assert.equal(execCalls[0].selectionEnd, 'hello fallback'.length);
        assert.equal(execCalls[0].ariaHidden, 'true');
        assert.equal(execCalls[0].hasMarker, true);
        await promise;

        assert.equal(body.children.includes(createdFields[0]), false, 'temporary textarea must be removed');
        assert.equal(doc.activeElement, canvas, 'previously focused element must be restored');
        assert.deepEqual(focusCalls.at(-1), { el: canvas, opts: { preventScroll: true } });
        assert.deepEqual(ranges.map(r => r.id), ['r1-clone']);

        // Rich copy with plain text degrades to plain text when navigator.clipboard is missing.
        execCalls.length = 0;
        await platform.writeClipboard('plain degraded', '<b>plain degraded</b>', new Uint8Array([1, 2, 3]));
        assert.equal(execCalls.length, 1);
        assert.equal(execCalls[0].value, 'plain degraded');

        // Image-only or HTML-only copy without plain text rejects without touching the DOM.
        execCalls.length = 0;
        const fieldsBefore = createdFields.length;
        await assert.rejects(
            platform.writeClipboard('', '', new Uint8Array([0x89, 0x50, 0x4e, 0x47])),
            /HTTPS/,
        );
        await assert.rejects(
            platform.writeClipboard('', '<b>html only</b>', new Uint8Array()),
            /HTTPS/,
        );
        assert.equal(execCalls.length, 0);
        assert.equal(createdFields.length, fieldsBefore);
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
    }
});

test('writeClipboard preserves text-control selection when restoring focus after legacy fallback', async () => {
    const origNav = globalThis.navigator;
    try {
        const { doc, body, execCalls } = installClipboardDom();
        const input = doc.createElement('input');
        input.value = 'abcdef';
        input.setSelectionRange(1, 4, 'backward');
        const origFocus = input.focus;
        // Simulate a browser that collapses control selection to 0 on refocus.
        input.focus = (opts) => {
            origFocus(opts);
            input.selectionStart = 0;
            input.selectionEnd = 0;
            input.selectionDirection = 'forward';
        };
        body.append(input);
        doc.activeElement = input;

        Object.defineProperty(globalThis, 'navigator', { value: {}, configurable: true });
        await platform.writeClipboard('copied text', '', new Uint8Array());

        assert.equal(execCalls.length, 1);
        assert.equal(doc.activeElement, input);
        assert.equal(input.selectionStart, 1);
        assert.equal(input.selectionEnd, 4);
        assert.equal(input.selectionDirection, 'backward');
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
    }
});

test('writeClipboard keeps async Clipboard API first, degrades rejected writes to plain text, and cleans up on refusal', async () => {
    const origNav = globalThis.navigator;
    const origCI = globalThis.ClipboardItem;
    try {
        class FakeClipboardItem {
            constructor(forms) { this.forms = forms; }
        }
        Object.defineProperty(globalThis, 'ClipboardItem', { value: FakeClipboardItem, configurable: true });

        // 1. Async writeText success never touches the DOM fallback.
        {
            const { execCalls } = installClipboardDom();
            const written = [];
            Object.defineProperty(globalThis, 'navigator', {
                value: { clipboard: { writeText: async (t) => { written.push(t); } } },
                configurable: true,
            });
            await platform.writeClipboard('modern plain', '', new Uint8Array());
            assert.deepEqual(written, ['modern plain']);
            assert.equal(execCalls.length, 0);
        }

        // 2. Async write success for rich content never touches writeText or DOM fallback.
        {
            const { execCalls } = installClipboardDom();
            let writeItems = null;
            let writeTextCalled = false;
            Object.defineProperty(globalThis, 'navigator', {
                value: {
                    clipboard: {
                        write: async (items) => { writeItems = items; },
                        writeText: async () => { writeTextCalled = true; },
                    },
                },
                configurable: true,
            });
            await platform.writeClipboard('hi', '<b>hi</b>', new Uint8Array([1, 2]));
            assert.equal(writeItems.length, 1);
            assert.ok(writeItems[0].forms['text/plain'] instanceof Blob);
            assert.ok(writeItems[0].forms['text/html'] instanceof Blob);
            assert.ok(writeItems[0].forms['image/png'] instanceof Blob);
            assert.equal(writeTextCalled, false);
            assert.equal(execCalls.length, 0);
        }

        // 3. Rejected async writeText falls back to temporary textarea copy.
        {
            const { doc, body, execCalls, createdFields } = installClipboardDom();
            const btn = doc.createElement('button');
            body.append(btn);
            doc.activeElement = btn;
            Object.defineProperty(globalThis, 'navigator', {
                value: {
                    clipboard: {
                        writeText: () => Promise.reject(Object.assign(new Error('denied'), { name: 'NotAllowedError' })),
                    },
                },
                configurable: true,
            });
            await platform.writeClipboard('after reject', '', new Uint8Array());
            assert.equal(execCalls.length, 1);
            assert.equal(execCalls[0].value, 'after reject');
            assert.equal(body.children.includes(createdFields[0]), false);
            assert.equal(doc.activeElement, btn);
        }

        // 4. Rich write rejected with NotAllowedError skips redundant writeText and degrades directly to DOM fallback.
        {
            const { execCalls } = installClipboardDom();
            let writeTextCalled = false;
            Object.defineProperty(globalThis, 'navigator', {
                value: {
                    clipboard: {
                        write: () => Promise.reject(Object.assign(new Error('denied'), { name: 'NotAllowedError' })),
                        writeText: async () => { writeTextCalled = true; },
                    },
                },
                configurable: true,
            });
            await platform.writeClipboard('rich denied', '<i>rich denied</i>', new Uint8Array());
            assert.equal(writeTextCalled, false);
            assert.equal(execCalls.length, 1);
            assert.equal(execCalls[0].value, 'rich denied');
        }

        // 5. Rich write rejected with non-permission error tries writeText first, then falls back if writeText rejects too.
        {
            const { execCalls } = installClipboardDom();
            const written = [];
            Object.defineProperty(globalThis, 'navigator', {
                value: {
                    clipboard: {
                        write: () => Promise.reject(Object.assign(new Error('unsupported mime'), { name: 'DataError' })),
                        writeText: async (t) => { written.push(t); },
                    },
                },
                configurable: true,
            });
            await platform.writeClipboard('mime fallback', '<p>mime fallback</p>', new Uint8Array());
            assert.deepEqual(written, ['mime fallback']);
            assert.equal(execCalls.length, 0);

            Object.defineProperty(globalThis, 'navigator', {
                value: {
                    clipboard: {
                        write: () => Promise.reject(Object.assign(new Error('unsupported mime'), { name: 'DataError' })),
                        writeText: () => Promise.reject(new Error('writeText rejected too')),
                    },
                },
                configurable: true,
            });
            await platform.writeClipboard('double fallback', '<p>double fallback</p>', new Uint8Array());
            assert.equal(execCalls.length, 1);
            assert.equal(execCalls[0].value, 'double fallback');
        }

        // 6. Image-only rich write rejected by clipboard.write rethrows without falling back to empty plain text.
        {
            const { execCalls } = installClipboardDom();
            let writeTextCalled = false;
            Object.defineProperty(globalThis, 'navigator', {
                value: {
                    clipboard: {
                        write: () => Promise.reject(new Error('image denied')),
                        writeText: async () => { writeTextCalled = true; },
                    },
                },
                configurable: true,
            });
            await assert.rejects(
                platform.writeClipboard('', '', new Uint8Array([0x89, 0x50, 0x4e, 0x47])),
                /image denied/,
            );
            assert.equal(writeTextCalled, false);
            assert.equal(execCalls.length, 0);
        }

        // 7. Refused execCommand('copy') (false or throw) rejects while still removing the temporary textarea
        //    and restoring focus and document selection.
        for (const mode of [{ execResult: false }, { execError: new Error('execCommand blocked') }]) {
            const { doc, body, createdFields, ranges } = installClipboardDom(mode);
            const canvas = doc.createElement('canvas');
            body.append(canvas);
            doc.activeElement = canvas;
            ranges.push({ id: 'keep', cloneRange() { return { id: 'keep-clone', cloneRange: this.cloneRange }; } });

            Object.defineProperty(globalThis, 'navigator', {
                value: { clipboard: { writeText: () => Promise.reject(new Error('NotAllowedError')) } },
                configurable: true,
            });
            await assert.rejects(
                platform.writeClipboard('cannot copy', '', new Uint8Array()),
                mode.execError ? /execCommand blocked/ : /Clipboard copy failed/,
            );
            assert.equal(createdFields.length, 1);
            assert.equal(body.children.includes(createdFields[0]), false, 'textarea must be removed on failure');
            assert.equal(doc.activeElement, canvas, 'focus must be restored on failure');
            assert.deepEqual(ranges.map(r => r.id), ['keep-clone'], 'selection must be restored on failure');
        }
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
        Object.defineProperty(globalThis, 'ClipboardItem', { value: origCI, configurable: true });
    }
});
