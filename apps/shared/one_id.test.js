import { test } from 'node:test';
import assert from 'node:assert/strict';

import { idForThisOne, whatIsOnTheForm } from './one_id.js';

/// A mint that counts, so a test can see how many ids were asked for.
function counting() {
  let at = 0;
  return () => {
    at += 1;
    return `id-${at}`;
  };
}

test('the same thing pressed twice is sent under one id', () => {
  const mint = counting();
  const shape = whatIsOnTheForm('rice', 5, 'CH-1');
  const first = idForThisOne(null, shape, mint);
  const again = idForThisOne(first, shape, mint);
  assert.equal(again.id, first.id, 'a repeat is the shop ignoring the second one');
  assert.equal(again, first);
});

test('a form changed after a lost reply is a different thing with a different id', () => {
  const mint = counting();
  // Booked, and the reply never came back.
  const booked = idForThisOne(null, whatIsOnTheForm('rice', 5), mint);
  // The owner changes what is on the form and presses again. Under the old id
  // the shop would call this a repeat of the first delivery and drop the lines.
  const changed = idForThisOne(booked, whatIsOnTheForm('rice', 6), mint);
  assert.notEqual(changed.id, booked.id);

  // And a person, which is worse: the shop upserts on the id, so the second
  // press under the first id renames the first person instead of adding one.
  const amina = idForThisOne(null, whatIsOnTheForm('Amina', 'cashier'), mint);
  const rahima = idForThisOne(amina, whatIsOnTheForm('Rahima', 'cashier'), mint);
  assert.notEqual(rahima.id, amina.id);
});

test('nothing kept mints once', () => {
  const mint = counting();
  const one = idForThisOne(null, whatIsOnTheForm('x'), mint);
  assert.equal(one.id, 'id-1');
});

test('what is on the form separates the parts rather than running them together', () => {
  // "ab" and "c" is not the same form as "a" and "bc", and a shape that joined
  // them with nothing between would call them the same delivery.
  assert.notEqual(whatIsOnTheForm('ab', 'c'), whatIsOnTheForm('a', 'bc'));
  // The order of the parts is part of it.
  assert.notEqual(whatIsOnTheForm('a', 'b'), whatIsOnTheForm('b', 'a'));
  // And the same form twice is the same string, whoever built it.
  assert.equal(whatIsOnTheForm('a', 1, null), whatIsOnTheForm('a', 1, null));
});
