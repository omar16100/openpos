// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about a pure function, and a dependency here
// is a dependency in the thing a shop runs.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { plain, whyTheRoundFailed } from './till.js';

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

/// The commonest failure of all is said in the shop's own words.
///
/// A round that cannot reach the shop arrives as whatever the browser calls it:
/// "Failed to fetch" in Chrome, "NetworkError when attempting to fetch
/// resource" in Firefox, "Load failed" in Safari. That is what a shopkeeper
/// read on the first failure of an outage, in the middle of a Bangla sentence,
/// on the one screen state this whole product exists for. Seen during an outage
/// walk: the line said "আটকে আছে: Failed to fetch".
test('a round that cannot reach the shop says so, whatever the browser calls it', () => {
  for (const said of [
    'TypeError: Failed to fetch',
    'NetworkError when attempting to fetch resource.',
    'Load failed',
    'Network request failed',
  ]) {
    assert.equal(whyTheRoundFailed(said, null), 'sync.cannot_reach_the_shop', said);
  }
});

test('a refusal with a name of its own is left to the dictionary', () => {
  // Those are worded from their code and their figures, which says more than
  // this could: "another sale already carries this receipt number" rather than
  // "not reaching the shop".
  assert.equal(whyTheRoundFailed('anything at all', 'duplicate-receipt'), null);
  // And a failure nobody has seen keeps the sentence it came with, for whoever
  // is sent to look at it.
  assert.equal(whyTheRoundFailed('the till did not say what to do next', null), null);
  assert.equal(whyTheRoundFailed(undefined, null), null);
});
