// One localStorage item per record. Edits never rewrite the preceding journal.
// Web Locks distinguish a crashed tab from a live tab without clocks or heartbeats.
const prefix = "soliddraft-command-journal/";
let current = null;
let nextRecord = 0;
let release = null;
let failed = false;
let recovery = null;

// Read-only discovery must not depend on acquiring or writing a new journal.
export async function prepare(exportReport) {
  if (!navigator.locks) throw new Error("Command journals require Web Locks on a secure origin");
  const previous = [];
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (key.startsWith(prefix) && key.endsWith("/0")) previous.push(key.slice(prefix.length, -2));
  }
  const offers = [];
  for (const candidate of previous.sort()) {
    await navigator.locks.request(prefix + candidate, {ifAvailable: true}, lock => {
      if (lock) offers.push(candidate);
    });
  }
  const root = document.createElement("section");
  root.id = "journal-recovery";
  root.setAttribute("aria-label", "Crash report journals");
  const title = document.createElement("h1");
  title.textContent = "Crash report journals";
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  message.textContent = "Starting the command journal. Editing is disabled until it is writable.";
  root.append(title, message);
  const empty = document.createElement("p");
  empty.dataset.empty = "";
  empty.textContent = "No saved crash report journals.";
  if (!offers.length) root.append(empty);
  const retry = document.createElement("button");
  retry.dataset.action = "retry";
  retry.textContent = "Retry journal startup";
  retry.disabled = true;
  for (const [index, id] of offers.entries()) {
    const row = document.createElement("p");
    row.append(`Session ${index + 1} `);
    const save = document.createElement("button");
    save.dataset.action = "export";
    save.textContent = "Save crash report journal";
    save.onclick = () => {
      try {
        exportReport(read(id));
        message.textContent = "Crash report downloaded. Review it before sharing. The saved journal is still here.";
      } catch (error) {
        message.textContent = String(error);
      }
    };
    const remove = document.createElement("button");
    remove.dataset.action = "discard";
    remove.textContent = "Discard journal";
    remove.onclick = async () => {
      remove.disabled = true;
      const wasDisabled = retry.disabled;
      retry.disabled = true;
      try {
        await discard(id);
        offers.splice(offers.indexOf(id), 1);
        row.remove();
        if (!offers.length) root.insertBefore(empty, retry);
        message.textContent = "Journal discarded. Retry startup when storage has enough free space.";
      } catch (error) {
        message.textContent = String(error);
        remove.disabled = false;
      } finally {
        retry.disabled = wasDisabled;
      }
    };
    row.append(save, " ", remove);
    root.append(row);
  }
  root.append(retry);
  document.body.append(root);
  recovery = {root, message, retry, offers};
  return JSON.stringify(offers);
}

export function retry(message) {
  recovery.message.textContent = `Editing is disabled because the command journal could not start: ${message}. Export or discard saved reports, or free browser storage, then retry.`;
  recovery.retry.disabled = false;
  return new Promise(resolve => {
    recovery.retry.onclick = () => {
      recovery.retry.disabled = true;
      resolve(JSON.stringify(recovery.offers));
    };
  });
}

export function ready() {
  recovery.root.remove();
  recovery = null;
}

function erase(id) {
  const start = `${prefix}${id}/`;
  const keys = [];
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (key.startsWith(start)) keys.push(key);
  }
  for (const key of keys) localStorage.removeItem(key);
}

export async function start(id) {
  if (!navigator.locks) throw new Error("Command journals require Web Locks on a secure origin");
  if (current !== null) throw new Error("A command journal is already active");
  await new Promise((ready, reject) => {
    navigator.locks.request(prefix + id, () => {
      current = id;
      nextRecord = 0;
      failed = false;
      ready();
      return new Promise(resolve => { release = resolve; });
    }).catch(reject);
  });
  addEventListener("pagehide", event => {
    if (!event.persisted && !failed) close(id);
  });
}

export function append(id, line) {
  if (id !== current || failed) throw new Error("Command journal is not writable");
  try {
    localStorage.setItem(`${prefix}${id}/${nextRecord}`, line);
    nextRecord++;
  } catch (error) {
    failed = true;
    throw error;
  }
}

export function crashed() { failed = true; }

export function read(id) {
  let text = "";
  for (let i = 0; ; i++) {
    const line = localStorage.getItem(`${prefix}${id}/${i}`);
    if (line === null) break;
    text += line;
  }
  if (!text) throw new Error("Command journal is no longer available");
  return text;
}

export function close(id) {
  if (id !== current) return;
  if (!failed) erase(id);
  current = null;
  release?.();
  release = null;
}

export async function discard(id) {
  await navigator.locks.request(prefix + id, {ifAvailable: true}, lock => {
    if (!lock) throw new Error("The command journal belongs to a running tab");
    erase(id);
  });
}
