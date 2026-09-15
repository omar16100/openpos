import { test } from 'node:test';
import assert from 'node:assert/strict';

import { WAITS, openingAgain } from './opening_again.js';
import { storageTrouble } from './storage_trouble.js';

/// The failure Chrome gives while the last window is still letting go.
function stillHeld() {
  const browser = new Error(
    "Failed to execute 'createSyncAccessHandle' on 'FileSystemFileHandle': Access Handles cannot be created if there is another open Access Handle or Writable stream associated with the same file.",
  );
  browser.name = 'NoModificationAllowedError';
  return storageTrouble(browser);
}

/// A device with no room, which is not going to change its mind.
function noRoom() {
  const browser = new Error('quota exceeded');
  browser.name = 'QuotaExceededError';
  return storageTrouble(browser);
}

/// Waits that cost nothing, and a record of how long was asked for.
function fakeClock() {
  const waited = [];
  return { waited, pause: async (ms) => void waited.push(ms) };
}

test('a store held by a window that has gone opens on the next try', async () => {
  // The handover: the page that had the files is being torn down while this one
  // is already asking. One short wait is all it takes, and the shop never sees
  // a message about it.
  let tries = 0;
  const clock = fakeClock();
  const opened = await openingAgain(
    async () => {
      tries += 1;
      if (tries === 1) throw stillHeld();
      return 'the ledger';
    },
    { pause: clock.pause },
  );
  assert.equal(opened, 'the ledger');
  assert.equal(tries, 2);
  assert.deepEqual(clock.waited, [WAITS[0]], 'and it waited once, briefly');
});

test('a store still held after every try refuses the way it always did', async () => {
  // A till genuinely open in another window. The screen has good words for
  // that, and this must not swallow them or delay them past reading.
  const clock = fakeClock();
  let tries = 0;
  await assert.rejects(
    () =>
      openingAgain(
        async () => {
          tries += 1;
          throw stillHeld();
        },
        { pause: clock.pause },
      ),
    (trouble) => trouble.code === 'till-open-elsewhere',
  );
  assert.equal(tries, WAITS.length + 1, 'one try, then one per wait');
  assert.deepEqual(clock.waited, WAITS);
  const total = WAITS.reduce((sum, one) => sum + one, 0);
  assert.ok(total <= 2_000, `waiting ${total}ms is longer than a person reads as a pause`);
});

test('a device with no room says so at once', async () => {
  // Not going to change its mind in a second, and what a shop does about it is
  // delete something. Waiting would only delay the sentence that helps.
  const clock = fakeClock();
  let tries = 0;
  await assert.rejects(
    () =>
      openingAgain(
        async () => {
          tries += 1;
          throw noRoom();
        },
        { pause: clock.pause },
      ),
    (trouble) => trouble.code === 'no-room-on-this-device',
  );
  assert.equal(tries, 1, 'asked once');
  assert.deepEqual(clock.waited, [], 'and waited for nothing');
});

test('a failure nobody has named is not retried either', async () => {
  // A reason this build has never seen keeps the browser's own sentence, and
  // whoever is sent to look should get it now rather than a second and a half
  // later.
  const clock = fakeClock();
  let tries = 0;
  await assert.rejects(
    () =>
      openingAgain(
        async () => {
          tries += 1;
          throw new Error('something else entirely');
        },
        { pause: clock.pause },
      ),
    /something else entirely/,
  );
  assert.equal(tries, 1);
});

test('a store that opens first time is not waited for at all', async () => {
  const clock = fakeClock();
  assert.equal(await openingAgain(async () => 'open', { pause: clock.pause }), 'open');
  assert.deepEqual(clock.waited, []);
});
