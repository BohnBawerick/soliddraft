// Run through chrome-devtools-axi run after opening the packaged application's origin.
// Storage, locks, Rust report redaction and the startup gate are all real.
const base = await page.eval(() => location.origin);
const prefix = 'soliddraft-command-journal/';
const id = '11111111-1111-4111-8111-111111111111';
function check(value, message) { if (!value) throw new Error(message); }
async function stored() {
  return page.eval(() => Object.fromEntries(Object.keys(localStorage).sort().map(key => [key, localStorage.getItem(key)])));
}
async function recover(exportFirst) {
  await page.open(`${base}/quota-seed-not-found`);
  console.log(await page.eval(() => {
    localStorage.clear();
    const prefix = 'soliddraft-command-journal/11111111-1111-4111-8111-111111111111/';
    localStorage.setItem(prefix + '0', '{"format":"soliddraft-journal/1"}\n');
    localStorage.setItem(prefix + '1', JSON.stringify({record:'command', sequence:1, document:1, command:{op:'execute', path:'/home/example/private.dwprt'}}) + '\n');
    localStorage.setItem(prefix + '2', JSON.stringify({record:'result', sequence:1, error:'x'.repeat(65536)}) + '\n');
    let n = 0;
    for (let size = 65536; size >= 1; size = Math.floor(size / 2)) {
      while (true) {
        try { localStorage.setItem(`quota-fill-${n}`, 'x'.repeat(size)); n++; }
        catch (error) { if (error.name !== 'QuotaExceededError') throw error; break; }
      }
    }
    try {
      localStorage.setItem('soliddraft-command-journal/22222222-2222-4222-8222-222222222222/0', '{"format":"soliddraft-journal/1"}\n');
      throw new Error('Quota still accepts a journal header');
    } catch (error) {
      if (error.name !== 'QuotaExceededError') throw error;
      return `Header probe: ${error.name}; ${localStorage.length} storage items`;
    }
  }));
  const original = await stored();
  await page.open(base);
  await page.wait('#journal-recovery button[data-action="retry"]:not(:disabled)');
  await page.eval(() => location.reload());
  await page.wait('#journal-recovery button[data-action="retry"]:not(:disabled)');
  console.log(await page.snapshot());
  check(JSON.stringify(await stored()) === JSON.stringify(original), 'Failed startup changed saved records');
  check(await page.eval(() => !document.querySelector('#status').classList.contains('hidden')), 'Editing started without a writable journal');
  // A second failure must release the failed startup lock and preserve every byte.
  await page.click('#journal-recovery button[data-action="retry"]');
  await page.wait('#journal-recovery button[data-action="retry"]:not(:disabled)');
  check(JSON.stringify(await stored()) === JSON.stringify(original), 'Retry changed saved records');
  if (exportFirst) {
    // Observe the actual download Blob without replacing the export or redaction code.
    await page.eval(() => {
      const create = URL.createObjectURL.bind(URL);
      URL.createObjectURL = blob => { window.reportDownload = blob; return create(blob); };
    });
    await page.click('#journal-recovery button[data-action="export"]');
    const text = await page.eval(async () => window.reportDownload.text());
    const lines = text.trimEnd().split('\n').map(line => JSON.parse(line));
    check(lines[0].format === 'soliddraft-journal/1' && lines[1].sequence === 1 && lines[2].error.length === 65536, 'Export lost a complete record');
    check(lines[1].command.path === '<path-1>.dwprt' && !text.includes('/home/example'), 'Export did not redact personal paths');
    check(JSON.stringify(await stored()) === JSON.stringify(original), 'Export removed or rewrote a report');
    // Export does not free storage. Simulate the user freeing unrelated site storage.
    await page.eval(() => {
      for (const key of Object.keys(localStorage)) if (key.startsWith('quota-fill-')) localStorage.removeItem(key);
    });
  } else {
    await page.click('#journal-recovery button[data-action="discard"]');
    await page.wait('#journal-recovery [data-empty]');
    const remaining = await stored();
    const expected = Object.fromEntries(Object.entries(original).filter(([key]) => !key.startsWith(prefix + id + '/')));
    check(JSON.stringify(remaining) === JSON.stringify(expected), 'Discard removed unrelated data or left report records');
  }
  await page.click('#journal-recovery button[data-action="retry"]');
  await page.wait('#status.hidden', 60000);
  check(await page.eval(() => !document.querySelector('#journal-recovery')), 'Recovery gate remained after journal startup');
  const after = await stored();
  check(Object.keys(after).some(key => key.startsWith(prefix) && !key.startsWith(prefix + id + '/') && key.endsWith('/0')), 'Editing started without a persisted new header');
  if (exportFirst) {
    for (const [key, value] of Object.entries(original).filter(([key]) => key.startsWith(prefix + id + '/'))) check(after[key] === value, 'Successful startup removed an exported report');
  }
  console.log(exportFirst ? 'PASS full quota, reload, redacted export, free space, successful startup; report retained' : 'PASS full quota, reload, explicit discard, successful startup; unrelated storage retained');
}
await recover(true);
await recover(false);
