// Talking to the till worker.
//
// One promise per command, matched by id, because a barcode scanner can fire
// faster than a round trip and replies that arrive out of order would otherwise
// render the wrong basket.

const pending = new Map();
let nextId = 1;
let worker = null;

function ensureWorker() {
  if (worker) return worker;
  worker = new Worker(new URL('./till.worker.js', import.meta.url), { type: 'module' });
  worker.onmessage = (event) => {
    const { id, ok, view, info, error } = event.data;
    const waiting = pending.get(id);
    if (!waiting) return;
    pending.delete(id);
    if (ok) waiting.resolve({ view, info });
    else waiting.reject(new Error(error));
  };
  return worker;
}

function send(kind, payload) {
  const id = nextId++;
  ensureWorker().postMessage({ id, kind, payload });
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

/// Open the till. `durable: false` keeps everything in memory.
export function open(tenant, terminal, durable = true) {
  return send('open', { tenant, terminal, durable });
}

/// Run one command and get the view back.
export function run(command) {
  return send('run', command);
}

/// Point the till at a server. No credential passes through here: the till holds
/// its own, beside its ledger, and hands it over with each request it builds.
export function connect(server) {
  return send('connect', { server });
}

/// Trade an enrolment code for a credential.
export function enrol(code, nowMs) {
  return send('enrol', { code, now_ms: nowMs });
}

/// One round of the sync loop.
export function sync(nowMs) {
  return send('sync', { now_ms: nowMs });
}
