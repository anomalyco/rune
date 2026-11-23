// Functional browser proof for web-hello. Requires Playwright and its Chromium browser.
import assert from 'node:assert/strict';
import { realpathSync } from 'node:fs';
import { readFile, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { resolve, extname, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { inflateSync } from 'node:zlib';
import { inputSmoke } from './web-input-smoke.mjs';
import { clockSmoke } from './web-clock-smoke.mjs';

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const CHANNELS_BY_COLOR_TYPE = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };
const LINUX_WEBGPU_ARGS = [
    '--enable-unsafe-swiftshader',
    '--enable-features=Vulkan',
    '--use-vulkan=swiftshader',
    '--use-angle=swiftshader',
    '--use-webgpu-adapter=swiftshader',
    '--ignore-gpu-blocklist',
];

export function chromiumArgs(platform = process.platform) {
    return [
        '--enable-unsafe-webgpu',
        ...(platform === 'darwin' ? ['--use-angle=metal'] : platform === 'linux' ? LINUX_WEBGPU_ARGS : []),
    ];
}

function paethPredictor(a, b, c) {
    const p = a + b - c;
    const pa = Math.abs(p - a);
    const pb = Math.abs(p - b);
    const pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

function unfilterScanline(filterType, raw, rowStart, stride, bpp, curr, prev) {
    switch (filterType) {
        case 0:
            curr.set(raw.subarray(rowStart, rowStart + stride));
            return;
        case 1:
            for (let x = 0; x < stride; x++) curr[x] = (raw[rowStart + x] + (x >= bpp ? curr[x - bpp] : 0)) & 0xff;
            return;
        case 2:
            for (let x = 0; x < stride; x++) curr[x] = (raw[rowStart + x] + prev[x]) & 0xff;
            return;
        case 3:
            for (let x = 0; x < stride; x++) {
                const a = x >= bpp ? curr[x - bpp] : 0;
                curr[x] = (raw[rowStart + x] + ((a + prev[x]) >> 1)) & 0xff;
            }
            return;
        case 4:
            for (let x = 0; x < stride; x++) {
                const a = x >= bpp ? curr[x - bpp] : 0;
                const b = prev[x];
                const c = x >= bpp ? prev[x - bpp] : 0;
                curr[x] = (raw[rowStart + x] + paethPredictor(a, b, c)) & 0xff;
            }
            return;
        default:
            throw new Error(`unsupported PNG filter type ${filterType}`);
    }
}

export function inspectPngUniformity(png) {
    const buf = Buffer.isBuffer(png) ? png : Buffer.from(png.buffer, png.byteOffset, png.byteLength);
    if (buf.length < 8 || !buf.subarray(0, 8).equals(PNG_SIGNATURE)) throw new Error('invalid PNG signature');
    let width = 0, height = 0, bitDepth = 0, colorType = -1, interlace = -1;
    let hasIhdr = false, hasIend = false;
    let palette = null, trns = null;
    const idat = [];
    for (let offset = 8; offset + 12 <= buf.length; ) {
        const length = buf.readUInt32BE(offset);
        const type = buf.toString('ascii', offset + 4, offset + 8);
        const dataStart = offset + 8;
        const dataEnd = dataStart + length;
        if (dataEnd + 4 > buf.length) throw new Error(`truncated PNG chunk ${type}`);
        if (type === 'IHDR') {
            if (length !== 13) throw new Error('invalid PNG IHDR length');
            width = buf.readUInt32BE(dataStart);
            height = buf.readUInt32BE(dataStart + 4);
            bitDepth = buf[dataStart + 8];
            colorType = buf[dataStart + 9];
            if (buf[dataStart + 10] !== 0 || buf[dataStart + 11] !== 0) {
                throw new Error('unsupported PNG compression or filter method');
            }
            interlace = buf[dataStart + 12];
            hasIhdr = true;
        } else if (type === 'PLTE') {
            palette = buf.subarray(dataStart, dataEnd);
        } else if (type === 'tRNS') {
            trns = buf.subarray(dataStart, dataEnd);
        } else if (type === 'IDAT') {
            idat.push(buf.subarray(dataStart, dataEnd));
        } else if (type === 'IEND') {
            hasIend = true;
            break;
        }
        offset = dataEnd + 4;
    }
    if (!hasIhdr || !hasIend || width === 0 || height === 0 || idat.length === 0) throw new Error('incomplete PNG');
    if (interlace !== 0) throw new Error(`unsupported PNG interlace method ${interlace}`);
    const channels = CHANNELS_BY_COLOR_TYPE[colorType];
    if (!channels) throw new Error(`unsupported PNG color type ${colorType}`);
    if (![1, 2, 4, 8, 16].includes(bitDepth)) throw new Error(`unsupported PNG bit depth ${bitDepth}`);
    if (colorType === 3 && (!palette || palette.length < 3 || palette.length % 3 !== 0)) {
        throw new Error('indexed PNG missing valid PLTE chunk');
    }

    const bitsPerPixel = channels * bitDepth;
    const bpp = Math.max(1, Math.ceil(bitsPerPixel / 8));
    const stride = Math.ceil((width * bitsPerPixel) / 8);
    const raw = inflateSync(Buffer.concat(idat));
    if (raw.length < height * (1 + stride)) throw new Error('truncated PNG image data');

    let prev = new Uint8Array(stride);
    let curr = new Uint8Array(stride);
    let first = null;
    const paletteColor = index => {
        const base = index * 3;
        if (base + 2 >= palette.length) throw new Error('PNG palette index out of range');
        const alpha = trns ? (index < trns.length ? trns[index] : 255) : null;
        return alpha === null
            ? [palette[base], palette[base + 1], palette[base + 2]]
            : [palette[base], palette[base + 1], palette[base + 2], alpha];
    };
    const subByteSample = (row, x) => {
        const perByte = 8 / bitDepth;
        const shift = 8 - bitDepth * ((x % perByte) + 1);
        return (row[Math.floor(x / perByte)] >> shift) & ((1 << bitDepth) - 1);
    };

    for (let y = 0; y < height; y++) {
        const rowOffset = y * (1 + stride);
        unfilterScanline(raw[rowOffset], raw, rowOffset + 1, stride, bpp, curr, prev);
        if (colorType === 3) {
            for (let x = 0; x < width; x++) {
                const idx = bitDepth === 8 ? curr[x] : subByteSample(curr, x);
                const c = paletteColor(idx);
                if (first === null) first = c;
                else if (c.some((v, k) => v !== first[k])) return { uniform: false, width, height };
            }
        } else if (bitDepth < 8) {
            for (let x = 0; x < width; x++) {
                const sample = subByteSample(curr, x);
                if (first === null) first = [sample];
                else if (sample !== first[0]) return { uniform: false, width, height };
            }
        } else {
            if (first === null) first = curr.slice(0, bpp);
            for (let i = 0; i < stride; i += bpp) {
                for (let k = 0; k < bpp; k++) {
                    if (curr[i + k] !== first[k]) return { uniform: false, width, height };
                }
            }
        }
        [prev, curr] = [curr, prev];
    }
    const color = colorType === 3 || bitDepth < 8
        ? first
        : bitDepth === 16
            ? Array.from({ length: channels }, (_, k) => (first[k * 2] << 8) | first[k * 2 + 1])
            : Array.from(first);
    return { uniform: true, width, height, color };
}

export function assertRenderedPng(png, label = 'screenshot') {
    const result = inspectPngUniformity(png);
    assert.equal(
        result.uniform,
        false,
        `${label} is a single solid color (${result.color?.join(', ')}) — canvas did not draw`,
    );
}

async function main() {
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
        chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH, args: chromiumArgs() }));
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
        const beforePan = await page.screenshot({ path: `${output}/web-dark.png` });
        assertRenderedPng(beforePan, 'web-dark.png');
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
        await page.waitForFunction(previous => window.__proof.submits > previous, releaseSubmits, { timeout: 3000 })
            .catch(() => assert.fail('touch release continues momentum'));
        const afterPan = await page.screenshot();
        assertRenderedPng(afterPan, 'after-pan screenshot');
        assert.ok(!beforePan.equals(afterPan), 'touch pan moves rendered content');
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
        await page.keyboard.press('ControlOrMeta+A');
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
        await linked.waitForURL(/rust-lang\.org/, { waitUntil: 'domcontentloaded' });
        assert.ok(linked.url().includes('rust-lang.org'));
        assert.equal(await linked.evaluate(() => window.opener), null);
        await linked.close();
        await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
        await page.setViewportSize({ width: 430, height: 540 });
        await page.waitForFunction(() => document.querySelector('canvas')?.height === 1620);
        await page.waitForTimeout(400);
        const lightShot = await page.screenshot({ path: `${output}/web-light-keyboard-size.png` });
        assertRenderedPng(lightShot, 'web-light-keyboard-size.png');
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
}

function isMain(metaUrl) {
    if (!process.argv[1]) return false;
    try {
        return realpathSync.native(resolve(process.argv[1])) === realpathSync.native(fileURLToPath(metaUrl));
    } catch {
        return false;
    }
}

if (isMain(import.meta.url)) {
    await main();
}
