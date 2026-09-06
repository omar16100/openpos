// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about a pure function, and a dependency here
// is a dependency in the thing a shop runs.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { saving } from './records.js';

test('a new record gets a fresh id', () => {
  const where = saving(null, () => 'MINTED');
  assert.equal(where.id, 'MINTED');
});

test('a correction is addressed to the record it corrects', () => {
  // The bug this exists for: minting here writes a second item beside the first,
  // and the shop then has two of the same rice at two prices.
  const where = saving({ id: 'EXISTING' }, () => 'MINTED');
  assert.equal(where.id, 'EXISTING');
});

test('an id is not minted when one is not needed', () => {
  let minted = 0;
  saving({ id: 'EXISTING' }, () => {
    minted += 1;
    return 'MINTED';
  });
  assert.equal(minted, 0, 'calling mint unconditionally is how the bug looks');
});

test('a correction keeps what nobody edited', () => {
  // Sending the default would put a withdrawn item back on the shelf and wipe a
  // cost the shop cannot recover, from a form that only changed a name.
  const where = saving({ id: 'EXISTING', active: false, cost_minor: 34_400 }, () => 'MINTED', {
    active: true,
    cost_minor: 0,
  });
  assert.equal(where.active, false);
  assert.equal(where.cost_minor, 34_400);
});

test('a new record takes the defaults for the same fields', () => {
  const where = saving(null, () => 'MINTED', { active: true, cost_minor: 0 });
  assert.equal(where.active, true);
  assert.equal(where.cost_minor, 0);
});

test('a field the record does not carry falls back rather than going missing', () => {
  // An older record from before a field existed, or one the server does not
  // send. Undefined would travel as an absent field and be read as false, which
  // withdraws something nobody withdrew.
  const where = saving({ id: 'EXISTING' }, () => 'MINTED', { active: true });
  assert.equal(where.active, true);
});

test('a carried field that is legitimately false stays false', () => {
  // The guard above must not swallow a real false, which is the whole point of
  // carrying `active` in the first place.
  const where = saving({ id: 'EXISTING', active: false }, () => 'MINTED', { active: true });
  assert.equal(where.active, false);
});
