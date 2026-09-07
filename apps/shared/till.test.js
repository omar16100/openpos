// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about a pure function, and a dependency here
// is a dependency in the thing a shop runs.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { plain } from './till.js';

test('a reactive proxy cannot be posted to a worker, and a copy of it can', () => {
  // What Svelte hands back for anything read out of the view. Posting one
  // throws "could not be cloned", the command never runs, and the screen shows
  // a message about postMessage instead of doing what was asked. That is what
  // happened to every action a supervisor allows: the core named the action,
  // the screen sent it back as it stands, and the send failed.
  const asItComesBack = new Proxy({ action: 'sell_beyond_stock' }, {});
  assert.throws(() => structuredClone(asItComesBack));

  const copy = plain({ op: 'authorise', action: asItComesBack });
  assert.deepEqual(structuredClone(copy), {
    op: 'authorise',
    action: { action: 'sell_beyond_stock' },
  });
});

test('what is not an object is passed through untouched', () => {
  // The large payloads are strings: a catalogue page, a hex body from the
  // server. Copying those through JSON would be a copy of a copy on the one
  // path that carries any volume.
  const hex = 'deadbeef'.repeat(64);
  assert.equal(plain(hex), hex);
  assert.equal(plain(7), 7);
  assert.equal(plain(null), null);
  assert.equal(plain(undefined), undefined);
});
