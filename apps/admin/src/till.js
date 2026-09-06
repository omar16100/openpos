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
    if (ok) {
      waiting.resolve({ view, info });
      return;
    }
    // The view rides along on the failure, because a request that failed still
    // changed what the till knows and a caller that catches this should be able
    // to show it.
    const refusal = new Error(error);
    refusal.view = view;
    waiting.reject(refusal);
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

/// Trade an enrolment code for a credential. Answers with the identity the code
/// gave this device, which is what a till must then be opened as.
export function enrol(code) {
  return send('enrol', { code });
}

/// Store the credential in a till that has just been opened with that identity.
export function adoptToken(token) {
  return send('adopt', { token });
}

/// One round of the sync loop.
export function sync(nowMs) {
  return send('sync', { now_ms: nowMs });
}

/// Ask the core to build a back-office request, post it, and hand back what
/// came out. The same three moves as everything else, so the back office knows
/// no more about the protocol than the till does.
export function admin(request, nowMs) {
  return send('admin', { request, now_ms: nowMs });
}
