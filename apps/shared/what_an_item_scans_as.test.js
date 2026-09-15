import { test } from 'node:test';
import assert from 'node:assert/strict';

import { barcodesKept } from './what_an_item_scans_as.js';

test('correcting an item keeps the barcodes it already scanned as', () => {
  // The bug: the form loaded the first barcode and saved that one alone, so a
  // shopkeeper changing a price deleted the rest. The cashier found out at the
  // counter, holding a box the shop said it had never heard of.
  assert.deepEqual(barcodesKept('8690000000002', ['8690000009991']), [
    '8690000000002',
    '8690000009991',
  ]);
});

test('the one in the box comes first, because it is the one shown next time', () => {
  assert.deepEqual(barcodesKept('111', ['222', '333']), ['111', '222', '333']);
});

test('a number typed that the item already had is kept once', () => {
  assert.deepEqual(barcodesKept('222', ['222', '333']), ['222', '333']);
});

test('an empty box is an item with no barcode, not an item with an empty one', () => {
  assert.deepEqual(barcodesKept('', []), []);
  assert.deepEqual(barcodesKept('   ', ['222']), ['222'], 'and the others stay');
});

test('a space at the end of what somebody typed is their keyboard', () => {
  assert.deepEqual(barcodesKept(' 8690000000002 ', []), ['8690000000002']);
});

test('nothing but a string is a barcode', () => {
  assert.deepEqual(barcodesKept(null, ['222']), ['222']);
  assert.deepEqual(barcodesKept(undefined, []), []);
});
