import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { movedPrice, repriced } from './repricing.js';

test('a price moves and lands on a taka a shopkeeper would say', () => {
  // 430.00 plus five percent is 451.50, which is not a price anybody quotes.
  assert.equal(movedPrice(43_000, 5), 45_200);
  assert.equal(movedPrice(43_000, -5), 40_900);
  assert.equal(movedPrice(10_000, 0), 10_000);
});

test('nothing lands below a taka', () => {
  // A rounding that reached zero would put something on the shelf for nothing.
  assert.equal(movedPrice(100, -99), 100);
  assert.equal(movedPrice(50, -50), 100);
});

test('what a run of items would become, for reading before agreeing', () => {
  const items = [
    { id: 'rice', name: 'Rice Miniket 5kg', price_minor: 43_000 },
    { id: 'soap', name: 'Soap', price_minor: 3_500 },
  ];
  const rows = repriced(items, 10);
  assert.deepEqual(rows, [
    { id: 'rice', name: 'Rice Miniket 5kg', was_minor: 43_000, now_minor: 47_300 },
    { id: 'soap', name: 'Soap', was_minor: 3_500, now_minor: 3_900 },
  ]);
});

test('something the shop has never priced is not moved by a percentage', () => {
  // Zero times anything is zero, and a list offering a shelf full of one-taka
  // goods is a list somebody agrees to by accident.
  const rows = repriced([{ id: 'x', name: 'Nobody priced this', price_minor: 0 }], 10);
  assert.deepEqual(rows, []);
});

test('a change that moves nothing is not offered as a change', () => {
  // A percentage too small to shift the nearest taka leaves the price where it
  // is, and a preview full of unchanged rows hides the ones that moved.
  const rows = repriced([{ id: 'rice', name: 'Rice', price_minor: 43_000 }], 0.05);
  assert.deepEqual(rows, []);
});

test('no percentage is no list', () => {
  assert.deepEqual(repriced([{ id: 'rice', name: 'Rice', price_minor: 43_000 }], 0), []);
  assert.deepEqual(repriced(undefined, Number.NaN), []);
});
