import { test } from 'node:test';
import assert from 'node:assert/strict';

import { pinFrom, LEAST_DIGITS } from './pin.js';

test('a PIN is digits, and four of them at least', () => {
  assert.equal(pinFrom('1234'), '1234');
  assert.equal(pinFrom('907361'), '907361');
  assert.equal(pinFrom('0000'), '0000', 'which digits is the shop’s business');
  assert.equal(pinFrom('123'), null, 'three is not four');
  assert.equal(pinFrom(''), null);
});

test('a PIN with a letter in it is one nobody can type at a counter', () => {
  // The bug: the box said "PIN, four digits or more", the check counted the
  // characters and nothing else, and the screen then told the shop that Walk
  // Letter Pin could sign in once the tills refreshed. The PIN box at a till is
  // `inputmode` numeric, so the keyboard on a phone or a tablet is a number pad
  // and that person could never have signed in on one. The owner setting it is
  // at a desk with a full keyboard, which is why nobody notices.
  assert.equal(pinFrom('abcd'), null);
  assert.equal(pinFrom('12ab'), null);
  assert.equal(pinFrom('1234x'), null);
});

test('a PIN is not trimmed, because a space is not a digit', () => {
  // Trimming would set a PIN whose first character a cashier cannot see and
  // would never think to type.
  assert.equal(pinFrom(' 1234'), null);
  assert.equal(pinFrom('1234 '), null);
  assert.equal(pinFrom('12 34'), null);
});

test('nothing but a string is a PIN', () => {
  assert.equal(pinFrom(1234), null);
  assert.equal(pinFrom(null), null);
  assert.equal(pinFrom(undefined), null);
});

test('the wording on the screen is the rule this keeps', () => {
  // Four, and the phrase says four. If one moves the other has to.
  assert.equal(LEAST_DIGITS, 4);
});
