import { test } from 'node:test';
import assert from 'node:assert/strict';

import { holdTheStore, holdWhileOpening } from './holding_the_store.js';

/// A lock manager that behaves like the browser's: one holder at a time, and
/// whoever is waiting is granted it when the holder lets go.
function aLockManager() {
    const queues = new Map();
    return {
        waiting: (name) => (queues.get(name) ?? []).length,
        request(name, _options, work) {
            const queue = queues.get(name) ?? [];
            queues.set(name, queue);
            return new Promise((settle, refuse) => {
                const run = () => {
                    Promise.resolve(work())
                        .then(settle, refuse)
                        .finally(() => {
                            queue.shift();
                            queue[0]?.();
                        });
                };
                queue.push(run);
                if (queue.length === 1) run();
            });
        },
    };
}

test('a store nobody holds is taken at once', async () => {
    const locks = aLockManager();
    const letGo = await holdTheStore('T1', { locks });
    assert.equal(typeof letGo, 'function');
    letGo();
});

test('a store the last window has not let go of is waited for', async () => {
    // The handover: the page that had the files is being torn down while this
    // one is already asking. A lock is held by a context rather than by a file,
    // and the browser releases it when that context dies, so waiting here is
    // waiting for something that is certain to happen.
    const locks = aLockManager();
    const first = await holdTheStore('T1', { locks });
    let second = null;
    const waiting = holdTheStore('T1', { locks, waitMs: 1_000 }).then((got) => {
        second = got;
    });
    await new Promise((settle) => setTimeout(settle, 20));
    assert.equal(second, null, 'not while somebody else has it');

    first();
    await waiting;
    assert.equal(typeof second, 'function', 'and taken as soon as they let go');
    second();
});

test('a store that is honestly open elsewhere is given up on and said', async () => {
    // The other case, and it is real: a till open in a second window will never
    // let go, and a screen that waits for ever there is worse than one that
    // says so. The shop gets the sentence it already had, which is the right
    // sentence for that case.
    const locks = aLockManager();
    const first = await holdTheStore('T1', { locks });
    const started = Date.now();
    const second = await holdTheStore('T1', { locks, waitMs: 60 });
    assert.equal(second, null);
    assert.ok(Date.now() - started >= 50, 'it waited before saying so');
    first();
});

test('a page that gave up does not leave the lock held behind it', async () => {
    // What a naive version does: the request is still outstanding, so when the
    // holder lets go the lock is handed to a page that has already told the
    // shop it could not have it, and the next window waits for ever on a page
    // that is not using it.
    const locks = aLockManager();
    const first = await holdTheStore('T1', { locks });
    assert.equal(await holdTheStore('T1', { locks, waitMs: 30 }), null);

    first();
    await new Promise((settle) => setTimeout(settle, 30));
    const third = await holdTheStore('T1', { locks, waitMs: 200 });
    assert.equal(typeof third, 'function', 'the lock came back to the next asker');
    third();
});

test('a browser with no locks is no worse off than before this existed', async () => {
    const letGo = await holdTheStore('T1', { locks: undefined });
    assert.equal(typeof letGo, 'function');
    letGo();
});

test('letting go twice is not an error', async () => {
    const locks = aLockManager();
    const letGo = await holdTheStore('T1', { locks });
    letGo();
    letGo();
    const again = await holdTheStore('T1', { locks, waitMs: 200 });
    assert.equal(typeof again, 'function');
    again();
});

test('a store that could not be opened is not left held', async () => {
  // The bug: the lock was taken, opening the files threw, and the lock stayed
  // held by that worker for as long as its page lived. Every attempt after it
  // then said the till was open in another window on this device, with no other
  // window open, and the only way out was restarting the tablet.
  const locks = aLockManager();
  await assert.rejects(
    holdWhileOpening('T1', () => { throw new Error('no room on this device'); }, { locks }),
    /no room on this device/,
    'the reason is what reaches the screen',
  );

  const after = await holdTheStore('T1', { locks, waitMs: 50 });
  assert.notEqual(after, null, 'and the next attempt is granted the store');
});

test('a store that opened is held until it is let go', async () => {
  const locks = aLockManager();
  const held = await holdWhileOpening('T1', () => ['a handle'], { locks });
  assert.deepEqual(held.opened, ['a handle']);
  assert.equal(await holdTheStore('T1', { locks, waitMs: 50 }), null, 'somebody has it');

  held.letGo();
  assert.notEqual(await holdTheStore('T1', { locks, waitMs: 50 }), null, 'and now nobody does');
});

test('a store that is somebody else\'s is refused without opening anything', async () => {
  const locks = aLockManager();
  const first = await holdTheStore('T1', { locks });
  assert.notEqual(first, null);

  let tried = false;
  const refused = await holdWhileOpening('T1', () => { tried = true; return []; }, { locks, waitMs: 40 });
  assert.equal(refused, null, 'the sentence on the screen is true here');
  assert.equal(tried, false, 'and nothing was opened behind somebody else\'s lock');
});
