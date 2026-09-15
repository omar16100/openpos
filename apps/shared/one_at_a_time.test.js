import { test } from 'node:test';
import assert from 'node:assert/strict';

import { oneAtATime } from './one_at_a_time.js';

/// Work that finishes when it is told to, which is how a slow shop behaves.
function heldOpen() {
  let release;
  const done = new Promise((settle) => {
    release = settle;
  });
  let started = 0;
  return {
    started: () => started,
    release: () => release(),
    work: async () => {
      started += 1;
      await done;
      return 'a round';
    },
  };
}

test('a tick that arrives while the last round is out is dropped', () => {
  // The lease case: the shop is answering slowly, the interval fires again, and
  // a second round asks for another block of five hundred receipt numbers while
  // the first grant has not been applied. The till keeps the block it is using
  // and one in reserve, so the one in the middle is stranded and the shop's
  // printed numbers jump.
  const slow = heldOpen();
  const round = oneAtATime(slow.work);

  const first = round();
  round();
  round();
  assert.equal(slow.started(), 1, 'one round is in the air, not three');
  slow.release();
  return first;
});

test('the next tick after a round comes back runs', async () => {
  const slow = heldOpen();
  const round = oneAtATime(slow.work);
  const first = round();
  slow.release();
  assert.equal(await first, 'a round');
  await round();
  assert.equal(slow.started(), 2, 'the door is open again');
});

test('a round that fails does not shut the door behind it', async () => {
  // The failure worth a test of its own. A till that stops syncing without
  // anybody being told is what this whole design is arranged against, and a
  // flag left set by a thrown round would do exactly that: every tick after it
  // returns at once, for ever, and the screen goes on saying the shop was
  // reached at whatever time that was.
  let tries = 0;
  const round = oneAtATime(async () => {
    tries += 1;
    throw new Error('the shop could not be reached');
  });

  await assert.rejects(round, /could not be reached/);
  await assert.rejects(round, /could not be reached/);
  assert.equal(tries, 2, 'the second tick still ran');
});

test('a dropped tick says so rather than looking like a result', async () => {
  const slow = heldOpen();
  const round = oneAtATime(slow.work, { skipped: 'not this time' });
  const first = round();
  assert.equal(await round(), 'not this time');
  slow.release();
  assert.equal(await first, 'a round');
});
