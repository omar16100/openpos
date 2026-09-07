import test from 'node:test';
import assert from 'node:assert/strict';

import {
  fileable,
  milliFrom,
  startSheet,
  summary,
  unusable,
  without,
  writeLine,
} from './counting.js';

let minted = 0;
const mint = () => `ID${(minted += 1)}`;

test('an emptied box is a shelf nobody counted, not a shelf found empty', () => {
  let sheet = startSheet(1_788_600_000_000);
  sheet = writeLine(sheet, 'rice', '12', mint);
  assert.equal(summary(sheet).counted, 1);

  // Number('') is zero. Booking that would report every shelf somebody cleared
  // the box on as empty, which looks exactly like a real finding.
  sheet = writeLine(sheet, 'rice', '', mint);
  assert.equal(summary(sheet).counted, 0);
  assert.deepEqual(fileable(sheet), []);
});

test('a line keeps its id when the number is corrected', () => {
  let sheet = startSheet(0);
  sheet = writeLine(sheet, 'rice', '12', mint);
  const first = fileable(sheet)[0].id;
  sheet = writeLine(sheet, 'rice', '13', mint);
  assert.equal(fileable(sheet)[0].id, first, 'so a resent batch is not a second count');
  assert.equal(fileable(sheet)[0].qty_milli, 13000);
});

test('a half-typed number stays in the sheet and is not sent', () => {
  let sheet = startSheet(0);
  sheet = writeLine(sheet, 'rice', '12', mint);
  sheet = writeLine(sheet, 'oil', '1.', mint);

  assert.equal(fileable(sheet).length, 1, 'only the one that is a quantity');
  assert.deepEqual(unusable(sheet), ['oil'], 'and the screen can say which');
  assert.deepEqual(summary(sheet), { counted: 1, wrong: 1, total: 2 });
});

test('what is filed comes out of the sheet and the rest stays to be tried again', () => {
  let sheet = startSheet(0);
  sheet = writeLine(sheet, 'rice', '12', mint);
  sheet = writeLine(sheet, 'oil', '4', mint);
  sheet = writeLine(sheet, 'dal', '7', mint);

  // One batch went, the connection died before the next. What was accepted is
  // gone from the sheet and what was not is still there to carry on with.
  const batch = fileable(sheet).slice(0, 2);
  sheet = without(sheet, batch);

  assert.equal(summary(sheet).counted, 1);
  assert.equal(fileable(sheet)[0].item_id, 'dal');
});
