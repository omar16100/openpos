import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { daysOfStock, notMoving, runningLow } from './buying.js';

const A_WEEK = 7 * 86_400_000;

test('the shelf that runs out first is first', () => {
  // Rice sells ten a day with thirty on the shelf; soap sells one a day with
  // fifty. Soap sold less and lasts longer, which is the whole point: a list of
  // what sold most puts rice second and tells the shopkeeper nothing.
  const sold = [
    { item: 'rice', qty_milli: 70_000 },
    { item: 'soap', qty_milli: 7_000 },
  ];
  const onHand = { rice: { qty_milli: 30_000 }, soap: { qty_milli: 50_000 } };

  const rows = daysOfStock(sold, onHand, A_WEEK);
  assert.deepEqual(
    rows.map((row) => row.item),
    ['rice', 'soap'],
  );
  assert.equal(rows[0].days_left, 3, 'thirty at ten a day');
  assert.equal(rows[1].days_left, 50, 'fifty at one a day');
});

test('what sold nothing is not on a buying list', () => {
  // It is dead stock or something the shop does not really sell. Either way it
  // lasts forever, and sorting by that would bury everything that matters.
  const rows = daysOfStock(
    [
      { item: 'rice', qty_milli: 70_000 },
      { item: 'calendars', qty_milli: 0 },
    ],
    { rice: { qty_milli: 30_000 }, calendars: { qty_milli: 40_000 } },
    A_WEEK,
  );
  assert.deepEqual(
    rows.map((row) => row.item),
    ['rice'],
  );
});

test('an empty shelf is the top of the list, not the bottom', () => {
  const rows = daysOfStock(
    [
      { item: 'rice', qty_milli: 70_000 },
      { item: 'oil', qty_milli: 7_000 },
    ],
    { rice: { qty_milli: 30_000 } },
    A_WEEK,
  );
  assert.equal(rows[0].item, 'oil', 'the shop holds no figure for it at all');
  assert.equal(rows[0].days_left, 0);
  assert.equal(rows[0].on_hand_milli, 0);
});

test('a shelf reading below zero is shown as it is', () => {
  // Goods left without a delivery behind them. Rounding it up to zero would
  // hide the thing worth looking at.
  const rows = daysOfStock(
    [{ item: 'rice', qty_milli: 70_000 }],
    { rice: { qty_milli: -2_000 } },
    A_WEEK,
  );
  assert.equal(rows[0].on_hand_milli, -2_000);
  assert.equal(rows[0].days_left, 0);
});

test('running low is what falls under the days the shop asked about', () => {
  const sold = [
    { item: 'rice', qty_milli: 70_000 },
    { item: 'soap', qty_milli: 7_000 },
  ];
  const onHand = { rice: { qty_milli: 30_000 }, soap: { qty_milli: 50_000 } };

  assert.deepEqual(
    runningLow(sold, onHand, A_WEEK, 7).map((row) => row.item),
    ['rice'],
    'three days is under a week and fifty is not',
  );
  assert.deepEqual(runningLow(sold, onHand, A_WEEK, 2), [], 'nothing is that close');
});

test('a window of nothing does not divide by it', () => {
  const rows = daysOfStock([{ item: 'rice', qty_milli: 1_000 }], { rice: { qty_milli: 1_000 } }, 0);
  assert.equal(Number.isFinite(rows[0].days_left), true);
});

test('what has not moved is listed, biggest money first', () => {
  const sold = [{ item: 'rice', qty_milli: 70_000 }];
  const held = {
    rice: { qty_milli: 30_000 },
    calendars: { qty_milli: 40_000 },
    hairclips: { qty_milli: 5_000 },
  };
  const costs = { rice: 38_000, calendars: 2_000, hairclips: 30_000 };

  const rows = notMoving(sold, held, costs);
  assert.deepEqual(
    rows.map((row) => row.item),
    ['hairclips', 'calendars'],
    'rice moved, and five hairclips at 300 beat forty calendars at 20',
  );
  assert.equal(rows[0].worth_minor, 150_000);
  assert.equal(rows[1].worth_minor, 80_000);
});

test('an empty shelf is not dead stock', () => {
  // Nothing is sitting there. It may be worth reordering, which is the other
  // list; it is not money on a shelf.
  const rows = notMoving([], { rice: { qty_milli: 0 }, oil: { qty_milli: -1_000 } }, {});
  assert.deepEqual(rows, []);
});

test('something the shop has never priced is still shown', () => {
  const rows = notMoving([], { calendars: { qty_milli: 40_000 } }, {});
  assert.equal(rows.length, 1);
  assert.equal(rows[0].worth_minor, 0);
  assert.equal(rows[0].costed, false, 'and says the figure is not a figure');
});
