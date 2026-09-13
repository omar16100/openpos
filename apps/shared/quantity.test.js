// Run with `node --test apps/shared/`. No test runner is installed on purpose:
// this is a handful of assertions about a pure function, and a dependency here
// is a dependency in the thing a shop runs.

import test from 'node:test';
import assert from 'node:assert/strict';

import { askedForOnThisTicket, howManyOnTheLine, milliFrom } from './quantity.js';

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

test('how many are on a line is how many, whichever way the ticket runs', () => {
  assert.equal(howManyOnTheLine(3000), 3000);
  assert.equal(howManyOnTheLine(-3000), 3000, 'three coming back is three');
  assert.equal(howManyOnTheLine(-1500), 1500, 'and so is a kilo and a half of it');
  assert.equal(howManyOnTheLine(0), 0);
  assert.equal(howManyOnTheLine(undefined), 0);
});

test('the sign is the ticket\'s, and is put on in one place', () => {
  // A sale asked for three gets three. A return asked for three gets three
  // below nothing, which is what makes its arithmetic the mirror of the sale it
  // undoes.
  assert.equal(askedForOnThisTicket(3000, false), 3000);
  assert.equal(askedForOnThisTicket(3000, true), -3000);
});

test('a quantity that already carries a sign is read as how many, not doubled back', () => {
  // The buttons hand back what the line holds, and a refund's line holds a
  // number below nothing. Read as "how many" it is three either way, so
  // stepping a refund up does not step it back towards a sale.
  assert.equal(askedForOnThisTicket(-3000, true), -3000);
  assert.equal(askedForOnThisTicket(-3000, false), 3000);
});

test('nothing left is nothing, and the screen takes the line off', () => {
  assert.equal(askedForOnThisTicket(0, true), -0);
  assert.equal(askedForOnThisTicket(0, false), 0);
  assert.equal(Math.abs(askedForOnThisTicket(0, true)), 0, 'a zero is a zero either way');
});
