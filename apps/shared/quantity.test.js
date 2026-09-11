// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about a pure function, and a dependency here
// is a dependency in the thing a shop runs.

import test from 'node:test';
import assert from 'node:assert/strict';

import { milliFrom } from './quantity.js';

test('quantities as anybody types them: a shelf being counted, loose rice being weighed', () => {
  assert.equal(milliFrom('12'), 12000);
  assert.equal(milliFrom('1.5'), 1500);
  assert.equal(milliFrom('0.250'), 250);
  assert.equal(milliFrom(' 40 '), 40000);
  assert.equal(milliFrom('0'), 0, 'a shelf found empty is a real finding');
});

test('what is not a quantity is refused rather than rounded', () => {
  assert.equal(milliFrom('1.5005'), null, 'no shop sells a thousandth of a thousandth');
  assert.equal(milliFrom('-2'), null);
  assert.equal(milliFrom('1e3'), null);
  assert.equal(milliFrom('two'), null);
  assert.equal(milliFrom(''), null);
});
