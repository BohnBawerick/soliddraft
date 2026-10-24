// One localStorage item per record. Edits never rewrite the preceding journal.
// Web Locks distinguish a crashed tab from a live tab without clocks or heartbeats.
const prefix = "soliddraft-command-journal/";
let current = null;
let nextRecord = 0;
let release = null;
let failed = false;

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
  const previous = [];
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (key.startsWith(prefix) && key.endsWith("/0")) {
      previous.push(key.slice(prefix.length, -2));
    }
  }
  const offered = [];
  for (const candidate of previous.sort()) {
    await navigator.locks.request(prefix + candidate, {ifAvailable: true}, lock => {
      if (lock) offered.push(candidate);
    });
  }
  addEventListener("pagehide", event => {
    if (!event.persisted && !failed) close(id);
  });
  return JSON.stringify(offered);
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
