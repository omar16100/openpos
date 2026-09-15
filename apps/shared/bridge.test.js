// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about how one file talks to a worker, and a
// dependency here is a dependency in the thing a shop runs.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { useWorker, run, keepSyncing, letGoOnTheWayOut, openAgainOnTheWayIn } from './till.js';

/// A worker that answers whatever it is told to, and can say something nobody
/// asked for. The bridge takes its worker from the outside precisely so this
/// can exist.
///
/// One for the whole file, because the bridge holds one worker per app and
/// makes it once: a second here would never be built, and the test would be
/// talking to the first while thinking it had a fresh one.
function fakeWorker() {
  const sent = [];
  const worker = {
    sent,
    onmessage: null,
    postMessage(message) {
      sent.push(message);
    },
    /// What the real worker does when a command finishes.
    answer(id, body) {
      this.onmessage({ data: { id, ...body } });
    },
    /// And what it now does every sync round, without being asked.
    announce(body) {
      this.onmessage({ data: { event: 'synced', ...body } });
    },
  };
  useWorker(() => worker);
  return worker;
}

const worker = fakeWorker();

test('a reply nobody asked for reaches the screen rather than the floor', async () => {
  // The sync loop lives in the worker, because a hidden tab's timers are
  // throttled to about once a minute and can stop altogether: a till that has
  // quietly stopped sending is the failure this design exists to prevent. Its
  // rounds arrive unasked, and before this the bridge dropped any message whose
  // id it did not recognise.
  const rounds = [];
  const started = keepSyncing((round) => rounds.push(round));
  worker.answer(worker.sent.at(-1).id, { ok: true, info: { looping: true } });
  await started;

  worker.announce({ ok: true, info: { did: 'push' }, view: { total_minor: 0 } });
  worker.announce({ ok: false, error: 'the shop is not answering', view: null });

  assert.equal(rounds.length, 2);
  assert.equal(rounds[0].ok, true);
  assert.equal(rounds[0].info.did, 'push');
  assert.equal(rounds[1].ok, false);
  assert.equal(rounds[1].error, 'the shop is not answering');
});

test('a round arriving mid-command does not answer the command', async () => {
  // The two channels share one port. A round landing while a scan is in flight
  // must not resolve the scan: the cashier would see the basket the round
  // described rather than the one they just added to.
  const rounds = [];
  const started = keepSyncing((round) => rounds.push(round));
  worker.answer(worker.sent.at(-1).id, { ok: true, info: { looping: true } });
  await started;

  const scanning = run({ op: 'scan', barcode: '8690000000001', qty_milli: 1000 });
  const id = worker.sent.at(-1).id;
  worker.announce({ ok: true, info: { did: 'pull' }, view: { total_minor: 0 } });
  worker.answer(id, { ok: true, view: { total_minor: 49450 } });

  const reply = await scanning;
  assert.equal(reply.view.total_minor, 49450, 'the scan got its own answer');
  assert.equal(rounds.length, 1, 'and the round went to the watcher');
});

test('a refusal the shop’s server gave arrives named, not just worded', async () => {
  // The server was the last place in this system that could only speak English.
  // Its refusals travel as an encoded ProtocolError, the core names them and
  // puts figures beside them, and the worker hangs those on the error it throws.
  //
  // An Error does not survive a postMessage with anything hung on it, which is
  // the part worth a test: the name and the figures have to be sent as their own
  // fields or the screen gets the English back and nothing to translate against.
  const asking = run({ op: 'catalogue', query: '', limit: 10 });
  worker.answer(worker.sent.at(-1).id, {
    ok: false,
    error: 'another item you sell already has the barcode 8901234567890',
    error_code: 'barcode-in-use',
    error_parts: { barcode: '8901234567890' },
    view: null,
  });

  const refusal = await asking.then(
    () => null,
    (error) => error,
  );
  assert.equal(refusal.code, 'barcode-in-use');
  assert.deepEqual(refusal.parts, { barcode: '8901234567890' });
  assert.match(refusal.message, /8901234567890/, 'and the English beside it, as the fallback');
});

test('a refusal with no name is still a refusal', async () => {
  // A browser that could not reach the shop at all, and a server one release
  // ahead sending a refusal this build has never heard of. Both arrive with a
  // sentence and nothing else, and the screen says the sentence: imperfect
  // rather than silent is the whole arrangement.
  const asking = run({ op: 'catalogue', query: '', limit: 10 });
  worker.answer(worker.sent.at(-1).id, {
    ok: false,
    error: 'Failed to fetch',
    error_code: null,
    error_parts: null,
    view: null,
  });

  const refusal = await asking.then(
    () => null,
    (error) => error,
  );
  assert.equal(refusal.code, undefined, 'nothing to key a dictionary on');
  assert.equal(refusal.message, 'Failed to fetch');
});

/// A window that fires the two events a page gets on its way out and back in.
///
/// Set on `globalThis` because the bridge asks for `window` at the moment it is
/// called, which is how the same file runs in a browser and under `node --test`.
function aWindow() {
  const listeners = new Map();
  const window = {
    addEventListener(name, listener) {
      listeners.set(name, [...(listeners.get(name) ?? []), listener]);
    },
    fire(name, event) {
      for (const listener of listeners.get(name) ?? []) listener(event);
    },
  };
  globalThis.window = window;
  return window;
}

test('a page the browser freezes lets the shop’s files go', () => {
  // The bug this is against: the page kept its files when it was frozen, on the
  // reasoning that it would come back exactly as it was and could not open them
  // again. A frozen page is alive and holds this terminal's store, and it
  // cannot answer anybody, so the next window waits out its whole patience and
  // is then told the till is open in another window on this device. There is no
  // such window. Nothing to close, nothing to switch to, and no way for the
  // person at the counter to learn that the window they are being sent to find
  // is a page the browser kept for the back button.
  const window = aWindow();
  letGoOnTheWayOut();
  const before = worker.sent.length;

  window.fire('pagehide', { persisted: true });

  assert.equal(worker.sent.length, before + 1, 'it said it was going');
  assert.equal(worker.sent.at(-1).kind, 'let_go');
});

test('a page closed for good lets them go too', () => {
  const window = aWindow();
  letGoOnTheWayOut();
  const before = worker.sent.length;

  window.fire('pagehide', { persisted: false });

  assert.equal(worker.sent.at(-1).kind, 'let_go');
  assert.equal(worker.sent.length, before + 1);
});

test('a page the browser brings back opens the ledger again', () => {
  // The other half, and the half the old reasoning said did not exist. A
  // restored page fires `pageshow` with `persisted` true, before anything can be
  // pressed, which is the chance to take the files back.
  const window = aWindow();
  let openedAgain = 0;
  openAgainOnTheWayIn(() => {
    openedAgain += 1;
  });

  window.fire('pageshow', { persisted: true });
  assert.equal(openedAgain, 1);
});

test('a page loading for the first time does not open twice', () => {
  // `pageshow` fires on an ordinary load as well, with `persisted` false. The
  // screen is already opening the ledger there, and a second open would be a
  // second worker command racing the first for the same files.
  const window = aWindow();
  let openedAgain = 0;
  openAgainOnTheWayIn(() => {
    openedAgain += 1;
  });

  window.fire('pageshow', { persisted: false });
  assert.equal(openedAgain, 0);
});

test('the page answers the worker asking whether anybody is there', () => {
  // A worker can outlive the page that made it: found with every window closed
  // and three tills still syncing to the shop. The worker asks, and lets the
  // shop's files go when nothing answers, so a page that is running has to
  // answer without any screen remembering to.
  const before = worker.sent.length;
  worker.onmessage({ data: { event: 'still_there' } });

  assert.equal(worker.sent.length, before + 1);
  assert.equal(worker.sent.at(-1).kind, 'still_here');
});

test('the answer does not reach a screen watching for sync rounds', async () => {
  const rounds = [];
  const started = keepSyncing((round) => rounds.push(round));
  worker.answer(worker.sent.at(-1).id, { ok: true, info: { looping: true } });
  await started;

  worker.onmessage({ data: { event: 'still_there' } });
  assert.equal(rounds.length, 0, 'it is not a round and must not be counted as one');
});
