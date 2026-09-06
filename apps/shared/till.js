// Talking to the till worker.
//
// One promise per command, matched by id, because a barcode scanner can fire
// faster than a round trip and replies that arrive out of order would otherwise
// render the wrong basket.

const pending = new Map();
let nextId = 1;
let worker = null;
let makeWorker = null;

/// How this app makes its worker.
///
/// Passed in rather than built here, because the bundler resolves a worker URL
/// against the file that writes it, and each app's worker entry is what hands
/// the core its own copy of the wasm under its own base path.
export function useWorker(factory) {
  makeWorker = factory;
}

function ensureWorker() {
  if (worker) return worker;
  if (!makeWorker) throw new Error('no worker was set up for this app');
  worker = makeWorker();
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
export function adoptToken(token, at_ms = Date.now()) {
  return send('adopt', { token, at_ms });
}

/// What a pasted bundle hashes to, by the same code that marked it on the
/// device it came from. Empty when the paste is not a bundle.
export function bundleMark(bundle) {
  return send('mark', { bundle });
}

/// Ask the core to build a back-office request, post it, and hand back what
/// came out. The same three moves as everything else, so the back office knows
/// no more about the protocol than the till does.
export function admin(request, nowMs) {
  return send('admin', { request, now_ms: nowMs });
}

/// What the sync loop is doing, in words a shopkeeper can act on.
///
/// Shared by the till and the back office, because a till that has stopped
/// reaching the shop must say so wherever it is looked at, and two copies of
/// this would be two chances to describe it as "idle".
export function describeSync(outcome) {
  if (outcome?.did) return outcome.did;
  const failures = outcome?.info?.after_failures ?? outcome?.after_failures ?? 0;
  if (failures === 0) return 'idle';
  const seconds = Math.max(1, Math.round((outcome?.info?.waited ?? outcome?.waited ?? 0) / 1000));
  // What is waiting to be sent is already on the screen, from the view. Saying
  // it again here meant two numbers taken at two moments, and they disagreed.
  return `not reaching the shop: trying again in ${seconds}s`;
}

/// One round of the sync loop.
export function sync(nowMs) {
  return send('sync', { now_ms: nowMs });
}
