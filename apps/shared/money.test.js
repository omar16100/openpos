import test from 'node:test';
import assert from 'node:assert/strict';

import { minorFrom } from './money.js';

test('taka and poisha as anybody types them', () => {
  assert.equal(minorFrom('100'), 10000);
  assert.equal(minorFrom('100.5'), 10050);
  assert.equal(minorFrom('100.50'), 10050);
  assert.equal(minorFrom('0.05'), 5);
  assert.equal(minorFrom(' 249.99 '), 24999);
});

test('what is not an amount is refused rather than rounded', () => {
  // Every one of these is a number to JavaScript and none of them is money.
  assert.equal(minorFrom('1e3'), null);
  assert.equal(minorFrom('0.001'), null);
  assert.equal(minorFrom('-50'), null);
  assert.equal(minorFrom('12,50'), null);
  assert.equal(minorFrom(''), null);
  assert.equal(minorFrom('abc'), null);
  assert.equal(minorFrom('50 taka'), null);
});

test('a poisha typed is a poisha sent', () => {
  // The case a float would lose: 0.29 * 100 is 28.999999999999996 in binary
  // floating point, and rounding it down would take a poisha off every one of
  // these that a shop takes in a year.
  assert.equal(minorFrom('0.29'), 29);
  assert.equal(minorFrom('1.15'), 115);
  assert.equal(minorFrom('494.50'), 49450);
});
