// Exercise the archived browser build under a project subpath, like GitHub Pages.
const { chromium } = require('playwright');
const http = require('node:http');
const fs = require('node:fs/promises');
const path = require('node:path');

async function main() {
  const root = path.resolve(process.argv[2]);
  const screenshot = process.argv[3];
  const mime = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    const relative = url.pathname.replace(/^\/soliddraft\//, '');
    const file = path.resolve(root, relative === '' ? 'index.html' : relative);
    if (!url.pathname.startsWith('/soliddraft/') || !file.startsWith(root + path.sep)) {
      response.writeHead(404).end();
      return;
    }
    try {
      const bytes = await fs.readFile(file);
      response.writeHead(200, { 'Content-Type': mime[path.extname(file)] || 'application/octet-stream' }).end(bytes);
    } catch {
      response.writeHead(404).end();
    }
  });
  let browser;
  try {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    browser = await chromium.launch({
      headless: true,
      args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'],
    });
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const errors = [];
    const wasm = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error' && /panic|SolidDraft stopped|could not start/i.test(message.text())) {
        errors.push(message.text());
      }
    });
    page.on('response', response => {
      if (new URL(response.url()).pathname.endsWith('.wasm')) wasm.push(response.status());
    });
    await page.goto(`http://127.0.0.1:${server.address().port}/soliddraft/?demo&preset=fast`);
    await page.waitForSelector('#status.hidden', { state: 'attached', timeout: 120_000 });
    await page.waitForFunction(() => {
      const canvas = document.querySelector('#soliddraft');
      return canvas && canvas.width >= 960 && canvas.height >= 600;
    });
    await page.screenshot({ path: screenshot });
    if (errors.length || !wasm.includes(200)) {
      throw new Error(JSON.stringify({ errors, wasm }));
    }
    console.log(JSON.stringify({ ready: true, wasm, screenshot, subpath: '/soliddraft/' }));
  } finally {
    if (browser) await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
