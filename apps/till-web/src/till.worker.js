// The till itself, in a dedicated worker.
//
// It lives here and not on the main thread for a reason the browser enforces:
// OPFS sync access handles cannot be created anywhere else. That constraint is
// the one the whole storage design was built around, and this file is where it
// is honoured.
//
// Nothing here decides anything. It opens files, forwards commands to the Rust
// core, and posts back what the core said. A rule that appeared in this file
// would be a rule the Android till does not have.

import init, { TillHandle } from '../public/pkg/openpos_bindings.js';

let till = null;
let handles = [];

/// Open the files the till keeps, creating them on a device's first morning.
async function openHandles(names) {
  const root = await navigator.storage.getDirectory();
  const opened = [];
  for (const name of names) {
    const file = await root.getFileHandle(name, { create: true });
    opened.push(await file.createSyncAccessHandle());
  }
  return opened;
}

async function open({ tenant, terminal, durable }) {
  await init();

  if (!durable) {
    // A demo, or a browser that refuses storage. Selling still works and
    // nothing survives a reload, and the screen says so rather than letting a
    // shopkeeper find out.
    till = TillHandle.openInMemory(tenant, terminal);
    if (!till) throw new Error('those identifiers are not valid ids');
    return { durable: false, storage: 'memory' };
  }

  handles = await openHandles(TillHandle.fileNames());

  // Run before the till opens, because the difference between a browser that
  // will not flush and a ledger that is corrupt decides what a shop should do
  // next, and after a failed open there is nothing left to ask.
  const check = TillHandle.selfTest(handles);
  if (check !== 'ok') throw new Error(`this device cannot store safely: ${check}`);

  till = TillHandle.openOpfs(handles, tenant, terminal);
  return { durable: true, storage: 'opfs' };
}

self.onmessage = async (event) => {
  const { id, kind, payload } = event.data;
  try {
    if (kind === 'open') {
      const info = await open(payload);
      postMessage({ id, ok: true, info, view: JSON.parse(till.view()) });
      return;
    }
    if (!till) throw new Error('the till is not open yet');

    // Every operation is one command through one entry point, so this file
    // never grows a branch per feature and cannot drift from the Android side.
    const reply = till.run(JSON.stringify(payload));
    postMessage({ id, ok: true, view: JSON.parse(reply) });
  } catch (error) {
    // Logged as well as posted. A failure that only travels back as a message
    // is a failure nobody can see the stack of, and the first one of these cost
    // an hour of looking at a screen that showed nothing wrong.
    console.error('[openpos] till worker', error);
    // Posted back rather than thrown. A worker that throws leaves the screen
    // showing the last thing that worked, which is the state a cashier would
    // ring the next customer into.
    postMessage({ id, ok: false, error: String(error.message ?? error) });
  }
};
