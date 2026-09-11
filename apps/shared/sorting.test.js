import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { UNSORTED, groupSold } from './sorting.js';

test('what sold is grouped by the words the shop uses', () => {
  const rows = [
    { item: 'a', qty_milli: 9_000, sales: 4 },
    { item: 'b', qty_milli: 5_000, sales: 3 },
    { item: 'c', qty_milli: 2_000, sales: 1 },
  ];
  const kinds = { a: 'Rice', b: 'Soap', c: 'Rice' };

  const groups = groupSold(rows, kinds);
  assert.deepEqual(
    groups.map((group) => group.kind),
    ['Rice', 'Soap'],
    'the shop’s own words, in order',
  );
  assert.deepEqual(
    groups[0].rows.map((row) => row.item),
    ['a', 'c'],
    'and the order inside a group is what moved most, as it arrived',
  );
});

test('what nobody has sorted is shown, and shown last', () => {
  const rows = [
    { item: 'a', qty_milli: 9_000, sales: 4 },
    { item: 'b', qty_milli: 5_000, sales: 3 },
    { item: 'c', qty_milli: 1_000, sales: 1 },
  ];
  // One sorted, one sorted under a blank, and one the device has no word for.
  const kinds = { a: 'Rice', b: '   ' };

  const groups = groupSold(rows, kinds);
  assert.deepEqual(
    groups.map((group) => group.kind),
    ['Rice', UNSORTED],
    'the pile nobody has got to is at the end rather than hidden',
  );
  assert.deepEqual(
    groups[1].rows.map((row) => row.item),
    ['b', 'c'],
  );
});

test('a shop that has sorted nothing still reads its own list', () => {
  const rows = [{ item: 'a', qty_milli: 9_000, sales: 4 }];
  const groups = groupSold(rows, {});
  assert.equal(groups.length, 1);
  assert.equal(groups[0].kind, UNSORTED);
  assert.equal(groups[0].rows.length, 1);
});

test('nothing sold is no groups, not one empty one', () => {
  assert.deepEqual(groupSold([], { a: 'Rice' }), []);
  assert.deepEqual(groupSold(undefined, undefined), []);
});
