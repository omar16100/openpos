import { test } from 'node:test';
import assert from 'node:assert/strict';

import { theBookByDay, theBookWithBalances } from './sales_book_rows.js';

/// A moment on a given local day, so the test says what it means wherever it
/// runs: the grouping is by the device's own day and so is this.
const at = (day, hour, minute = 0) => new Date(2026, 8, day, hour, minute).getTime();

const sale = (day, hour, qtyMilli) => ({ at_ms: at(day, hour), kind: 1, qty_milli: -qtyMilli });
const arrival = (day, hour, qtyMilli, reference) => ({
  at_ms: at(day, hour),
  kind: 2,
  qty_milli: qtyMilli,
  reference,
  supplier_name: 'Chittagong Rice Mills',
  supplier_bin: '004123456-0101',
});
const breakage = (day, hour, qtyMilli) => ({ at_ms: at(day, hour), kind: 3, qty_milli: qtyMilli });

test('a day of selling is one row', () => {
  const rows = theBookByDay([sale(4, 9, 3_000), sale(4, 14, 2_000), sale(4, 19, 1_000)]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].soldMilli, 6_000, 'six bags out, as a size');
  assert.deepEqual(rows[0].cameIn, []);
});

test('goods coming back that day reduce what went out', () => {
  // A return is a sale with its sign turned over, and the form has one column
  // for goods leaving the shelf.
  const rows = theBookByDay([sale(4, 9, 5_000), { at_ms: at(4, 17), kind: 1, qty_milli: 2_000 }]);
  assert.equal(rows[0].soldMilli, 3_000);
});

test('two deliveries in one day each keep their own invoice number', () => {
  const rows = theBookByDay([
    arrival(4, 8, 25_000, 'CRM/2026/220'),
    arrival(4, 16, 10_000, 'CRM/2026/221'),
    sale(4, 12, 4_000),
  ]);
  assert.equal(rows.length, 1, 'one day');
  assert.deepEqual(
    rows[0].cameIn.map((one) => one.reference),
    ['CRM/2026/220', 'CRM/2026/221'],
    'the form gives one row four columns for one purchase, so two purchases are two rows on the page',
  );
  assert.equal(rows[0].cameIn[0].supplierBin, '004123456-0101');
  assert.equal(rows[0].soldMilli, 4_000);
});

test('days come out oldest first, whatever order the shop answered in', () => {
  const rows = theBookByDay([sale(9, 11, 1_000), sale(2, 11, 1_000), arrival(5, 11, 3_000, 'X')]);
  assert.deepEqual(
    rows.map((row) => row.day),
    ['2026-09-02', '2026-09-05', '2026-09-09'],
  );
});

test('anything else that moved the shelf is kept, because the page has to add up', () => {
  const rows = theBookByDay([breakage(4, 10, -2_000)]);
  assert.equal(rows[0].correctedMilli, -2_000);
  assert.equal(rows[0].soldMilli, 0, 'a breakage is not a sale');
});

test('the balances run down the page from what it opened at', () => {
  const rows = theBookWithBalances(
    30_000,
    theBookByDay([
      arrival(4, 8, 25_000, 'CRM/2026/220'),
      sale(4, 12, 7_000),
      breakage(4, 18, -2_000),
      sale(5, 10, 3_000),
    ]),
  );

  assert.equal(rows[0].openingMilli, 30_000);
  assert.equal(rows[0].cameInMilli, 25_000);
  assert.equal(rows[0].totalMilli, 55_000, 'মোট is what was held plus what came in');
  assert.equal(rows[0].soldMilli, 7_000);
  assert.equal(rows[0].closingMilli, 46_000, 'less what went out and the two that broke');

  assert.equal(rows[1].openingMilli, 46_000, 'the next day opens where the last one closed');
  assert.equal(rows[1].closingMilli, 43_000);
});

test('a quiet month is no rows, and the page still opened at something', () => {
  assert.deepEqual(theBookByDay([]), []);
  assert.deepEqual(theBookWithBalances(30_000, []), []);
});
