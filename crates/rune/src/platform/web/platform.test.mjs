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

function installDom({ width = 390, height = 844, offsetLeft = 0, offsetTop = 0 } = {}) {
    const body = new FakeElement('body');
    const doc = Object.assign(new FakeEventTarget(), {
        body,
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
    class FakeResizeObserver {
        observe() {}
        disconnect() {}
    }

    Object.defineProperty(globalThis, 'document', { value: doc, configurable: true });
    Object.defineProperty(globalThis, 'window', { value: win, configurable: true });
    Object.defineProperty(globalThis, 'innerWidth', { value: width, configurable: true });
    Object.defineProperty(globalThis, 'innerHeight', { value: height, configurable: true });
    Object.defineProperty(globalThis, 'matchMedia', { value: makeMedia, configurable: true });
    Object.defineProperty(globalThis, 'ResizeObserver', { value: FakeResizeObserver, configurable: true });
    Object.defineProperty(globalThis, 'getComputedStyle', {
        value: () => ({ paddingTop: '0', paddingRight: '0', paddingBottom: '0', paddingLeft: '0' }),
        configurable: true,
    });
    return { doc, win, body, visualViewport };
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

test('viewport startup microtask emits one combined init snapshot, respects dispose, and keeps live events separate', async () => {
    const { doc, win, body } = installDom({ width: 390, height: 844 });
    const queries = new Map();
    Object.defineProperty(globalThis, 'matchMedia', {
        value: (query) => {
            const target = Object.assign(new FakeEventTarget(), {
                matches: query === '(prefers-reduced-motion: reduce)'
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
        hover: false,
        coarse: true,
        colors: [
            [1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12],
            [13, 14, 15], [16, 17, 18], [19, 20, 21], [22, 23, 24],
        ],
        focused: false,
        visible: false,
    }]);

    // Ongoing listener events continue to emit their individual kinds.
    events.length = 0;
    queries.get('(prefers-reduced-motion: reduce)').matches = false;
    queries.get('(prefers-reduced-motion: reduce)').dispatchEvent({ type: 'change' });
    queries.get('(forced-colors: active)').matches = false;
    queries.get('(forced-colors: active)').dispatchEvent({ type: 'change' });
    doc.hidden = false;
    doc.hasFocus = () => true;
    win.dispatchEvent({ type: 'focus' });
    doc.dispatchEvent({ type: 'visibilitychange' });
    assert.deepEqual(events, [
        { kind: 'motion', reduced: false },
        { kind: 'contrast', colors: null },
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
