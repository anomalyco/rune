import assert from 'node:assert/strict';
import test from 'node:test';

class FakeElement {
    constructor(tagName) {
        this.tagName = tagName.toUpperCase();
        this.style = { cssText: '', width: '', height: '', left: '', top: '' };
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
    assert.deepEqual(events.find(e => e.kind === 'viewport')?.visual, [0, 699, 699]);

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

test('writeClipboardImage creates ClipboardItem with Promise<Blob> synchronously and encodes off-turn', async () => {
    const origNav = globalThis.navigator;
    const origCI = globalThis.ClipboardItem;
    try {
        let writtenItems = null;
        let makePngCalled = false;
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

        const pngBytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3, 4]);
        const promise = platform.writeClipboardImage(() => {
            makePngCalled = true;
            return { png: pngBytes };
        });
        // Clipboard.write and ClipboardItem must have been called synchronously inside the gesture,
        // BEFORE makePng() runs on a later turn.
        assert.equal(makePngCalled, false, 'makePng must not run synchronously on the input turn');
        assert.equal(writtenItems?.length, 1);
        assert.ok(writtenItems[0].forms['image/png'] instanceof Promise);

        const blob = await promise;
        assert.equal(makePngCalled, true);
        assert.equal(blob.type, 'image/png');
        assert.deepEqual(new Uint8Array(await blob.arrayBuffer()), pngBytes);

        // Abort from makePng rejects without resolving a blob.
        await assert.rejects(
            platform.writeClipboardImage(() => ({ error: 'image exceeds limit' })),
            /image exceeds limit/,
        );

        // Browser clipboard rejection (e.g. outside user gesture) propagates.
        Object.defineProperty(globalThis, 'navigator', {
            value: { clipboard: { write: () => Promise.reject(new Error('NotAllowedError')) } },
            configurable: true,
        });
        await assert.rejects(
            platform.writeClipboardImage(() => ({ png: pngBytes })),
            /NotAllowedError/,
        );
    } finally {
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
        Object.defineProperty(globalThis, 'ClipboardItem', { value: origCI, configurable: true });
    }
});
