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
let token = null;

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

/// Post the bytes the core built, and hand back the bytes that came out.
///
/// The only thing this file knows about the protocol: where to send it and how
/// to carry a credential. Everything about what the bytes mean is in Rust.
async function post(path, bodyHex) {
  const body = new Uint8Array(bodyHex.length / 2);
  for (let i = 0; i < body.length; i += 1) {
    body[i] = parseInt(bodyHex.substr(i * 2, 2), 16);
  }
  const response = await fetch(server + path, {
    method: 'POST',
    headers: token ? { authorization: `Bearer ${token}` } : {},
    body,
  });
  if (!response.ok) throw new Error(`${path} answered ${response.status}`);
  const out = new Uint8Array(await response.arrayBuffer());
  return Array.from(out, (b) => b.toString(16).padStart(2, '0')).join('');
}

/// Trade a code for a credential. The one request that carries none.
async function enrol(code) {
  const reply = JSON.parse(till.run(JSON.stringify({ op: 'view' })));
  if (reply.error) throw new Error(reply.error);
  // The enrolment body is small and fixed, so it is the one place this file
  // builds a request itself rather than being handed one.
  const request = await fetch(server + '/v1/enrol', {
    method: 'POST',
    body: encodeEnrol(code),
  });
  if (!request.ok) throw new Error(`enrolment refused: ${request.status}`);
  const bytes = new Uint8Array(await request.arrayBuffer());
  return decodeEnrolToken(bytes);
}

/// postcard for `EnrolRequest { protocol: u16, code: String }`: a varint
/// protocol number, then a varint length and the code's bytes.
function encodeEnrol(code) {
  const text = new TextEncoder().encode(code);
  return new Uint8Array([1, text.length, ...text]);
}

/// postcard for `EnrolResponse { protocol, tenant, terminal, token }`. The
/// first three are varints of unknown width, so the token is found from the end:
/// it is the last field, a length and then that many bytes.
function decodeEnrolToken(bytes) {
  for (let at = 0; at < bytes.length; at += 1) {
    const length = bytes[at];
    if (at + 1 + length === bytes.length) {
      return new TextDecoder().decode(bytes.slice(at + 1));
    }
  }
  throw new Error('the enrolment reply was not the shape this build expects');
}

/// One round of the loop: ask, post, hand back.
async function syncOnce(nowMs) {
  const stepped = JSON.parse(
    till.run(JSON.stringify({ op: 'sync_step', online: navigator.onLine, now_ms: nowMs })),
  );
  // A step that could not even be decided is a failure, not a quiet decision to
  // do nothing. Treating the two alike is how a till stops syncing without
  // anybody being told, which is the failure this whole design is arranged
  // against.
  if (stepped.error) throw new Error(stepped.error);

  const step = stepped.step;
  if (!step) throw new Error('the till did not say what to do next');
  if (step.action === 'wait') return { waited: step.for_ms };

  try {
    const reply = await post(step.path, step.body);
    const applied = JSON.parse(
      till.run(JSON.stringify({ op: 'sync_apply', kind: step.kind, body: reply, now_ms: nowMs })),
    );
    if (applied.error) throw new Error(applied.error);
    return { did: step.kind, ...applied.applied };
  } catch (error) {
    // Told to the till rather than swallowed, so the backoff is the core's and
    // not this file's idea of one.
    till.run(JSON.stringify({ op: 'sync_failed', now_ms: nowMs }));
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
      token = payload.token ?? null;
      postMessage({ id, ok: true, info: { connected: token !== null } });
      return;
    }

    if (!till) throw new Error('the till is not open yet');

    if (kind === 'enrol') {
      token = await enrol(payload.code);
      postMessage({ id, ok: true, info: { token } });
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
    postMessage({ id, ok: false, error: String(error.message ?? error) });
  }
};
