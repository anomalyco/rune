// Functional browser proof for web-hello. Requires Playwright and its Chromium browser.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';
import { inputSmoke } from './web-input-smoke.mjs';
import { clockSmoke } from './web-clock-smoke.mjs';
const { chromium, webkit } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const engine = process.env.WEB_ENGINE || 'chromium';
const root = resolve(process.argv[2] || 'target/web');
const output = resolve(process.argv[3] || 'target/web-proof');
await mkdir(output, { recursive: true });
const server = createServer(async (request, response) => {
    const path = resolve(root, '.' + new URL(request.url, 'http://localhost').pathname.replace(/\/$/, '/index.html'));
    if (!path.startsWith(root + sep)) { response.writeHead(403).end(); return; }
    try {
        const bytes = await readFile(path);
        response.setHeader('Content-Type', ({ '.wasm': 'application/wasm', '.js': 'text/javascript', '.html': 'text/html' })[extname(path)] || 'application/octet-stream');
        response.end(bytes);
    } catch { response.writeHead(404).end(); }
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const browser = await (engine === 'webkit' ? webkit.launch({ headless: true }) :
    chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH,
        args: ['--enable-unsafe-webgpu', ...(process.platform === 'darwin' ? ['--use-angle=metal'] : [])] }));
const errors = [];
try {
    const context = await browser.newContext({ viewport: { width: 390, height: 760 }, deviceScaleFactor: 3,
        hasTouch: true, isMobile: true,
        permissions: engine === 'chromium' ? ['clipboard-read', 'clipboard-write'] : [], colorScheme: 'dark' });
    const page = await context.newPage();
    page.on('pageerror', error => { errors.push(error.message); console.log(error.message); });
    page.on('console', message => { if (message.type() === 'error') { errors.push(message.text()); console.log(message.text()); } });
    await page.addInitScript(() => {
        // Chromium's device emulation leaves devicePixelContentBoxSize at the host DPR.
        // Exercise winit's Safari fallback (CSS content box × emulated DPR) instead.
        delete ResizeObserverEntry.prototype.devicePixelContentBoxSize;
        window.__proof = { submits: 0, rafs: 0, gpuErrors: [] };
        const raf = window.requestAnimationFrame;
        window.requestAnimationFrame = callback => { window.__proof.rafs++; return raf.call(window, callback); };
        if (!globalThis.GPUQueue || !globalThis.GPUAdapter) return;
        const submit = GPUQueue.prototype.submit;
        GPUQueue.prototype.submit = function (...args) { window.__proof.submits++; return submit.apply(this, args); };
        const request = GPUAdapter.prototype.requestDevice;
        GPUAdapter.prototype.requestDevice = async function (...args) {
            const device = await request.apply(this, args);
            device.addEventListener('uncapturederror', e => window.__proof.gpuErrors.push(e.error.message));
            return device;
        };
    });
    const url = `http://127.0.0.1:${server.address().port}`;
    await page.goto(url);
    if (engine === 'webkit' && !await page.evaluate(async () => !!(await navigator.gpu?.requestAdapter()))) {
        console.log(JSON.stringify({ engine, available: false, reason: 'Headless WebKit has no WebGPU adapter' }));
    } else {
    if (!process.env.WEB_INPUT_ONLY) {
    await page.waitForFunction(() => window.__proof?.submits > 0, { timeout: 30000 });
    await page.waitForTimeout(1000); // Let the initial appearance finish before measuring idle.
    assert.deepEqual(await page.locator('canvas').evaluate(c => [c.width, c.height]), [1170, 2280]);
    const beforeIdle = await page.evaluate(() => window.__proof.submits);
    const beforeIdleRaf = await page.evaluate(() => window.__proof.rafs);
    await page.waitForTimeout(600);
    assert.equal(await page.evaluate(() => window.__proof.submits), beforeIdle, 'idle must not submit frames');
    assert.equal(await page.evaluate(() => window.__proof.rafs), beforeIdleRaf, 'idle must not request animation frames');
    await page.screenshot({ path: `${output}/web-dark.png` });
    const beforePan = await page.screenshot();
    const cdp = engine === 'chromium' ? await context.newCDPSession(page) : null;
    const touch = async (type, y) => cdp ? cdp.send('Input.dispatchTouchEvent', {
        type, touchPoints: type === 'touchEnd' ? [] : [{ x: 180, y }],
    }) : page.locator('canvas').dispatchEvent(({ touchStart: 'pointerdown', touchMove: 'pointermove', touchEnd: 'pointerup' })[type],
        { pointerType: 'touch', pointerId: 1, isPrimary: true, clientX: 180, clientY: y, buttons: type === 'touchEnd' ? 0 : 1 });
    await touch('touchStart', 180);
    for (let i = 1; i <= 10; i++) {
        await touch('touchMove', 180 + 30 * i);
        await page.waitForTimeout(16);
    }
    await touch('touchEnd', 480);
    const releaseSubmits = await page.evaluate(() => window.__proof.submits);
    await page.waitForTimeout(250);
    assert.ok(await page.evaluate(() => window.__proof.submits) > releaseSubmits, 'touch release continues momentum');
    assert.ok(!beforePan.equals(await page.screenshot()), 'touch pan moves rendered content');
    assert.notEqual(await page.evaluate(() => document.activeElement?.tagName), 'TEXTAREA', 'a pan must not focus an editor');
    await page.touchscreen.tap(100, 720);
    await page.waitForFunction(() => document.activeElement?.tagName === 'TEXTAREA');
    await page.keyboard.insertText('Bonjour 🦀');
    assert.equal(await page.locator('textarea').inputValue(), 'Bonjour 🦀');
    await page.keyboard.press('Enter');
    assert.equal(await page.locator('textarea').inputValue(), '');
    await page.touchscreen.tap(330, 36);
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'Bonjour 🦀', 'send updates the actual model');
    await page.touchscreen.tap(100, 720);
    await page.keyboard.insertText('x'.repeat(1200) + ' tail');
    assert.ok((await page.locator('textarea').inputValue()).length <= 1024, 'mirror is a sliding context');
    await page.locator('textarea').evaluate(field => {
        field.setSelectionRange(field.value.length - 4, field.value.length);
        field.setRangeText('corrected', field.selectionStart, field.selectionEnd, 'end');
        field.dispatchEvent(new InputEvent('input', { inputType: 'insertReplacementText', data: 'corrected' }));
    });
    await page.keyboard.press('Enter');
    await page.touchscreen.tap(330, 36);
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'x'.repeat(1200) + ' corrected');
    await page.touchscreen.tap(100, 720);
    const beforeMissingGlyph = await page.evaluate(() => window.__proof.submits);
    await page.locator('textarea').evaluate(field => {
        field.dispatchEvent(new CompositionEvent('compositionstart', { data: '' }));
        field.value = '日本語'; field.setSelectionRange(3, 3);
        field.dispatchEvent(new CompositionEvent('compositionupdate', { data: '日本語' }));
        field.dispatchEvent(new InputEvent('input', { data: '日本語', isComposing: true }));
        field.dispatchEvent(new CompositionEvent('compositionend', { data: '日本語' }));
    });
    assert.equal(await page.locator('textarea').inputValue(), '日本語');
    await page.waitForFunction(previous => window.__proof.submits > previous, beforeMissingGlyph);
    await page.keyboard.press('Meta+A');
    await page.keyboard.insertText('replacement');
    assert.equal(await page.locator('textarea').inputValue(), 'replacement');
    // Browser paste is independent of async clipboard-read permission.
    await page.locator('textarea').evaluate(field => {
        const clipboardData = new DataTransfer(); clipboardData.setData('text/plain', ' pasted');
        field.dispatchEvent(new ClipboardEvent('paste', { clipboardData, bubbles: true }));
    });
    assert.equal(await page.locator('textarea').inputValue(), 'replacement pasted');
    const popup = context.waitForEvent('page');
    await page.touchscreen.tap(90, 36);
    const linked = await popup;
    await linked.waitForLoadState('domcontentloaded');
    assert.ok(linked.url().includes('rust-lang.org'));
    assert.equal(await linked.evaluate(() => window.opener), null);
    await linked.close();
    await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
    await page.setViewportSize({ width: 430, height: 540 });
    await page.waitForFunction(() => document.querySelector('canvas')?.height === 1620);
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${output}/web-light-keyboard-size.png` });
    const gpuErrors = await page.evaluate(() => window.__proof.gpuErrors);
    assert.deepEqual(gpuErrors, []);
    assert.deepEqual(errors.filter(e => !e.includes('404')), []);
    console.log(JSON.stringify({ ok: true, engine, idleSubmits: 0, idleRafs: 0, dpr: 3, sizingPath: 'Safari fallback', checks: ['WebGPU render', 'idle', 'touch focus',
        'touch momentum', 'Unicode input', 'send', 'clipboard copy', 'sliding context', 'predictive replacement',
        'composition', 'selection replacement', 'paste', 'noopener link', 'theme', 'resize'], gpuErrors }));
    }
    await page.setViewportSize({ width: 390, height: 760 });
    await inputSmoke(page, url);
    // JPEG bytes go to the browser's codecs, PNG bytes to Rune's.
    await page.goto(`${url}/?image-proof`);
    await page.waitForFunction(() => window.__imageProof?.jpeg && window.__imageProof?.png);
    assert.deepEqual(await page.evaluate(() => window.__imageProof), { jpeg: { width: 12, height: 6 }, png: { width: 12, height: 6 } });
    // The cursor over them is the PNG at 2x, which the browser decodes and sizes in points.
    const cursor = () => page.evaluate(() => getComputedStyle(document.querySelector('canvas')).cursor);
    await page.mouse.move(76, 114);
    await page.waitForFunction(() => getComputedStyle(document.querySelector('canvas')).cursor.includes('image-set('));
    const css = await cursor();
    assert.match(css, /image-set\(url\("blob:[^"]+"\) 2(x|dppx)\) 1 2, auto$/, css);
    const size = await page.evaluate(async (url) => {
        const image = new Image();
        image.src = url;
        await image.decode();
        return [image.naturalWidth, image.naturalHeight];
    }, css.match(/blob:[^"]+/)[0]);
    assert.deepEqual(size, [12, 6]);
    await page.mouse.move(300, 700);
    await page.waitForFunction(() => getComputedStyle(document.querySelector('canvas')).cursor === 'auto');
    // Inter's full faces load from beside app.js once the first frame is up.
    await page.waitForFunction(() => performance.getEntriesByType('resource').filter(e => /\/Inter-\w+\.ttf$/.test(e.name) && e.responseEnd > 0).length === 3);
    assert.deepEqual(await page.evaluate(() => performance.getEntriesByType('resource')
        .filter(e => /\/Inter-\w+\.ttf$/.test(e.name)).map(e => e.responseStatus ?? 200)), [200, 200, 200]);
    console.log(JSON.stringify({ ok: true, checks: ['image bytes', 'image cursor', 'full Inter faces'] }));
    assert.deepEqual(await page.evaluate(() => window.__proof.gpuErrors), []);
    assert.deepEqual(errors.filter(e => !e.includes('404')), []);
    if (engine === 'chromium') { await clockSmoke(browser, url); console.log(JSON.stringify({ ok: true, checks: ['page clock'] })); }
    }
} finally {
    await browser.close();
    await new Promise(done => server.close(done));
}
