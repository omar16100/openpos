// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about how one file talks to a worker, and a
// dependency here is a dependency in the thing a shop runs.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { useWorker, run, keepSyncing } from './till.js';

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
