// Browser proof for luau-panel: the Luau panel renders in Rune's web build, rests at zero frames,
// scrolls its 1,000-row list, answers presses, and reports the interpreter's performance numbers.
// Requires Playwright. Usage: node web-proof.mjs [web dir] [output dir]
// (build first: scripts/web-build.sh luau-panel --out target/web-luau, with WASI_SDK_PATH set).
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = resolve(process.argv[2] || 'target/web-luau');
const output = resolve(process.argv[3] || 'target/web-luau-proof');
await mkdir(output, { recursive: true });
const server = createServer(async (request, response) => {
    if (request.url === '/favicon.ico') { response.writeHead(204).end(); return; }
    const path = resolve(root, '.' + new URL(request.url, 'http://localhost').pathname.replace(/\/$/, '/index.html'));
    if (!path.startsWith(root + sep)) { response.writeHead(403).end(); return; }
    try {
        const bytes = await readFile(path);
        response.setHeader('Content-Type', ({ '.wasm': 'application/wasm', '.js': 'text/javascript', '.html': 'text/html' })[extname(path)] || 'application/octet-stream');
        response.end(bytes);
    } catch { response.writeHead(404).end(); }
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH, args: ['--enable-unsafe-webgpu'] });
const errors = [];
const log = [];
try {
    const context = await browser.newContext({ viewport: { width: 720, height: 640 }, deviceScaleFactor: 1, colorScheme: 'dark',
        recordVideo: { dir: output, size: { width: 720, height: 640 } } });
    const page = await context.newPage();
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { log.push(`${message.type()}: ${message.text()}`); if (message.type() === 'error') errors.push(message.text()); });
    await page.addInitScript(() => {
        Error.stackTraceLimit = 100;
        window.__proof = { submits: 0, rafs: 0 };
        const raf = window.requestAnimationFrame;
        window.requestAnimationFrame = callback => { window.__proof.rafs++; return raf.call(window, callback); };
        if (!globalThis.GPUQueue) return;
        const submit = GPUQueue.prototype.submit;
        GPUQueue.prototype.submit = function (...args) { window.__proof.submits++; return submit.apply(this, args); };
    });
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.waitForFunction(() => window.__proof?.submits > 0, null, { timeout: 60000 }).catch(async error => {
        console.log(log.join('\n'), errors.join('\n'), await page.locator('body').innerText());
        throw error;
    });
    await page.waitForTimeout(1500);
    const counts = () => page.evaluate(() => ({ ...window.__proof }));
    const idle = async ms => { const a = await counts(); await page.waitForTimeout(ms); const b = await counts(); return { submits: b.submits - a.submits, rafs: b.rafs - a.rafs, ms }; };
    const result = { idle: await idle(3000) };
    await page.screenshot({ path: `${output}/01-loaded.png` });

    // Scroll the 1,000-row list: 120 wheel steps of 40 px, one per frame.
    const before = await counts();
    await page.mouse.move(360, 400);
    for (let i = 0; i < 120; i++) { await page.mouse.wheel(0, 40); await page.waitForTimeout(8); }
    await page.waitForTimeout(500);
    result.scroll = { submits: (await counts()).submits - before.submits };
    await page.screenshot({ path: `${output}/02-scrolled.png` });
    result.idleAfterScroll = await idle(2000);

    // Press a row, then Pulse (a task.wait countdown), then Ping (a signal).
    await page.mouse.click(300, 300);
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${output}/03-selected.png` });
    await page.mouse.click(110, 52);
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${output}/04-pulsing.png` });
    await page.waitForTimeout(1500);
    await page.mouse.click(160, 52);
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${output}/05-pinged.png` });
    result.idleAfterTasks = await idle(2000);

    // The interpreter's numbers, measured in this page by the same code as the native bench.
    result.bench = JSON.parse(await page.evaluate(async () => (await import('./app.js')).luau_bench()));
    result.errors = errors;
    await writeFile(`${output}/web-proof.json`, JSON.stringify(result, null, 2));
    await writeFile(`${output}/console.txt`, log.join('\n'));
    console.log(JSON.stringify(result, null, 2));
    assert.equal(result.idle.submits, 0, 'an idle page draws nothing');
    assert.ok(result.scroll.submits > 30, 'scrolling draws');
    assert.equal(result.idleAfterTasks.submits, 0, 'the page rests after its tasks finish');
    assert.deepEqual(errors, []);
    await context.close();
} finally {
    await browser.close();
    server.close();
}
