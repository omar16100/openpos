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
// Where the server is and what this device is allowed to say to it. Held here
// rather than passed with every command, because a credential that travels
// through the UI on every call is a credential that ends up in a log.
let server = null;

/// Open the files the till keeps, creating them on a device's first morning.
/// One directory per terminal.
///
/// Two apps served from one origin share an OPFS root, so the till and the back
/// office were opening the same files as different terminals. The journal's
/// owner check caught it and refused, which is what it is for, but the answer is
/// not to make them share more carefully: it is to give each device its own
/// store, named for the only thing that distinguishes them.
async function openHandles(names, terminal) {
  const root = await navigator.storage.getDirectory();
  const home = await root.getDirectoryHandle(terminal, { create: true });
  const opened = [];
  for (const name of names) {
    const file = await home.getFileHandle(name, { create: true });
    opened.push(await file.createSyncAccessHandle());
  }
  return opened;
}

async function open({ tenant, terminal, durable }) {
  await init();

  // One worker holds one till. Opening a second without releasing the first
  // leaves the files held, and the browser then refuses with a complaint about
  // access handles rather than anything to do with what went wrong. It happens
  // on the ordinary path: a device opens the store it remembers, then enrols
  // and opens the store it was told to.
  for (const handle of handles) {
    try {
      handle.close();
    } catch {
      // Already gone, which is the state we want.
    }
  }
  handles = [];
  till = null;

  if (!durable) {
    // A demo, or a browser that refuses storage. Selling still works and
    // nothing survives a reload, and the screen says so rather than letting a
    // shopkeeper find out.
    till = TillHandle.openInMemory(tenant, terminal);
    if (!till) throw new Error('those identifiers are not valid ids');
    return { durable: false, storage: 'memory' };
  }

  handles = await openHandles(TillHandle.fileNames(), terminal);

  try {
    // Run before the till opens, because the difference between a browser that
    // will not flush and a ledger that is corrupt decides what a shop should do
    // next, and after a failed open there is nothing left to ask.
    const check = TillHandle.selfTest(handles);
    if (check !== 'ok') throw new Error(`this device cannot store safely: ${check}`);

    till = TillHandle.openOpfs(handles, tenant, terminal);
    return { durable: true, storage: 'opfs' };
  } catch (error) {
    // A held file cannot be opened again, so a failed open that kept its
    // handles turns every retry into a complaint about access handles instead
    // of the reason it failed the first time. That cost twenty minutes.
    for (const handle of handles) {
      try {
        handle.close();
      } catch {
        // Already gone, which is the state we want.
      }
    }
    handles = [];
    throw error;
  }
}

/// Post the bytes the core built, and hand back the bytes that came out.
///
/// The only thing this file knows about the protocol: where to send it and how
/// to carry a credential. Everything about what the bytes mean is in Rust.
async function post(path, bodyHex, stepToken) {
  const body = new Uint8Array(bodyHex.length / 2);
  for (let i = 0; i < body.length; i += 1) {
    body[i] = parseInt(bodyHex.substr(i * 2, 2), 16);
  }
  const response = await fetch(server + path, {
    method: 'POST',
    // The credential comes with the step the core built, so this file never
    // holds one and cannot send a stale one.
    headers: stepToken ? { authorization: `Bearer ${stepToken}` } : {},
    body,
  });
  if (!response.ok) {
    // The status travels with the error, because the core decides what a status
    // means and this file decides nothing. A refusal of the credential and a
    // server that is merely down look identical from here, and only one of them
    // is worth retrying for the rest of the day.
    const refusal = new Error(`${path} answered ${response.status}`);
    refusal.status = response.status;
    throw refusal;
  }
  const out = new Uint8Array(await response.arrayBuffer());
  return Array.from(out, (b) => b.toString(16).padStart(2, '0')).join('');
}

/// One round of the loop: ask, post, hand back.
async function syncOnce(nowMs) {
  return carry(
    JSON.parse(
      till.run(JSON.stringify({ op: 'sync_step', online: navigator.onLine, now_ms: nowMs })),
    ),
    nowMs,
  );
}

/// Carry out whatever step the core described.
///
/// Shared by syncing and by enrolling, because they are the same three moves and
/// writing them twice is how the credential ends up attached in one place and
/// forgotten in the other.
async function carry(stepped, nowMs) {
  // A step that could not even be decided is a failure, not a quiet decision to
  // do nothing. Treating the two alike is how a till stops syncing without
  // anybody being told, which is the failure this whole design is arranged
  // against.
  if (stepped.error) throw new Error(stepped.error);

  const step = stepped.step;
  if (!step) throw new Error('the till did not say what to do next');
  if (step.action === 'wait') return { waited: step.for_ms };

  try {
    const reply = await post(step.path, step.body, step.token);
    const applied = JSON.parse(
      till.run(JSON.stringify({ op: 'sync_apply', kind: step.kind, body: reply, now_ms: nowMs })),
    );
    if (applied.error) throw new Error(applied.error);
    return { did: step.kind, ...applied.applied };
  } catch (error) {
    // Told to the till rather than swallowed, so the backoff is the core's and
    // not this file's idea of one. The status goes with it, or a device holding
    // a credential the server has revoked retries it until somebody notices the
    // sales are not arriving.
    till.run(
      JSON.stringify({ op: 'sync_failed', now_ms: nowMs, status: error.status ?? null }),
    );
    throw error;
  }
}

self.onmessage = async (event) => {
  const { id, kind, payload } = event.data;
  try {
    if (kind === 'open') {
      const info = await open(payload);
      postMessage({ id, ok: true, info, view: JSON.parse(till.view()) });
      return;
    }
    if (kind === 'connect') {
      server = payload.server;
      postMessage({ id, ok: true, info: { server } });
      return;
    }

    if (kind === 'enrol') {
      // Before any till exists. The code decides which terminal this device is,
      // and a till has to be opened as somebody: opening one as a guess first
      // is what made a second device present a credential for one terminal and
      // a request body for another.
      await init();
      const step = JSON.parse(TillHandle.enrolRequest(payload.code));
      if (step.error) throw new Error(step.error);
      const reply = await post(step.path, step.body, null);
      const credential = JSON.parse(TillHandle.readEnrolment(reply));
      if (credential.error) throw new Error(credential.error);
      postMessage({ id, ok: true, info: credential });
      return;
    }

    if (!till) throw new Error('the till is not open yet');

    if (kind === 'adopt') {
      const view = JSON.parse(till.adoptToken(payload.token));
      postMessage({ id, ok: true, view });
      return;
    }



    if (kind === 'admin') {
      const stepped = JSON.parse(
        till.run(JSON.stringify({ op: 'admin', request: payload.request })),
      );
      const outcome = await carry(stepped, payload.now_ms);
      postMessage({ id, ok: true, info: outcome, view: JSON.parse(till.view()) });
      return;
    }

    if (kind === 'sync') {
      const outcome = await syncOnce(payload.now_ms);
      postMessage({ id, ok: true, info: outcome, view: JSON.parse(till.view()) });
      return;
    }

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
    // The view goes back with the failure. A failed request still changes what
    // the till knows - a refused credential most of all - and a screen that only
    // gets a string cannot show any of it. That is how a device holding a
    // credential the server had revoked went on looking enrolled.
    let view = null;
    try {
      if (till) view = JSON.parse(till.view());
    } catch {
      // A till that cannot describe itself is past reporting anything, and the
      // error already on its way is the more useful of the two.
    }
    postMessage({ id, ok: false, error: String(error.message ?? error), view });
  }
};
