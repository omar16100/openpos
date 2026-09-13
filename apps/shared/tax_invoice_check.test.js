import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  goodsCameBack,
  linesTheFormCannotCarry,
  theFormCanCarry,
  theNoteWouldBeRefused,
} from './tax_invoice_check.js';

/// An ordinary line: a hundred taka of rice at fifteen percent.
const rice = { name: 'Rice Miniket 5kg', net_minor: 10_000, vat_minor: 1_500, vat_bp: 1_500 };

test('an ordinary sale goes on the form', () => {
  assert.equal(theFormCanCarry([rice]), true);
  assert.deepEqual(linesTheFormCannotCarry([rice]), []);
});

test('a discounted line whose tax is fixed to the listed price does not', () => {
  // The case, measured off this product's own screens: an item of 100.00 whose
  // tax is fixed to its listed price, with ten percent off the ticket. It is
  // charged for at 90.00 and taxed 15.00, because the discount comes out of the
  // shop's margin rather than off the tax. Fifteen percent of 90.00 is 13.50.
  // Both figures are right. The form has no column for the 100.00 the rate was
  // charged on, so an inspector multiplying the two columns it does have gets a
  // different answer from the one printed beside them.
  const listed = { name: 'Listed price 100.00', net_minor: 9_000, vat_minor: 1_500, vat_bp: 1_500 };
  assert.equal(theFormCanCarry([listed]), false);
  assert.deepEqual(
    linesTheFormCannotCarry([rice, listed]).map((line) => line.name),
    ['Listed price 100.00'],
    'and it says which line, because a shopkeeper has to know what to do about it',
  );
});

test('a poisha of rounding is not a disagreement', () => {
  // 33.33 at fifteen percent is 4.9995, which rounds to 5.00. An exact
  // comparison would refuse the form for a line that is right.
  assert.equal(theFormCanCarry([{ net_minor: 3_333, vat_minor: 500, vat_bp: 1_500 }]), true);
  assert.equal(theFormCanCarry([{ net_minor: 3_333, vat_minor: 499, vat_bp: 1_500 }]), true);
  assert.equal(
    theFormCanCarry([{ net_minor: 3_333, vat_minor: 502, vat_bp: 1_500 }]),
    false,
    'two poisha is somebody else’s arithmetic',
  );
});

test('a line carrying no tax at all goes on the form', () => {
  // Zero rated and exempt: nothing is charged and nothing is declared, which
  // agrees with any rate times nothing.
  assert.equal(theFormCanCarry([{ net_minor: 10_000, vat_minor: 0, vat_bp: 0 }]), true);
});

test('goods coming back are checked the same way', () => {
  // A refund's figures are the sale's with their signs turned over, and the
  // rounding has to turn over with them or a mirrored line reads as a
  // disagreement.
  assert.equal(theFormCanCarry([{ net_minor: -3_333, vat_minor: -500, vat_bp: 1_500 }]), true);
});

test('a sale with nothing on it is not refused', () => {
  assert.equal(theFormCanCarry([]), true);
  assert.equal(theFormCanCarry(), true);
});

test('the form is for a supply, and goods coming back are not one', () => {
  assert.equal(goodsCameBack({ total_minor: 55_200 }), false);
  assert.equal(goodsCameBack({ total_minor: -9_000 }), true, 'a refund runs below nothing');
  // The till and the back office hold the same sale under two spellings.
  assert.equal(goodsCameBack({ totalMinor: -9_000 }), true);
  // Nothing rung is not a refund.
  assert.equal(goodsCameBack({ total_minor: 0 }), false);
  assert.equal(goodsCameBack(undefined), false);
  assert.equal(goodsCameBack({}), false);
});

test('a credit note over 5,000 of tax has to name who it is for', () => {
  const named = { name: 'Rahman Wholesale', bin: '123456789-0202' };
  // Section 52(1)(f) and 52(2): over five thousand of VAT the note names the
  // buyer, and one that does not cannot be used to claim the adjustment.
  assert.equal(theNoteWouldBeRefused({ vat_minor: -600_000 }, null), true);
  assert.equal(theNoteWouldBeRefused({ vat_minor: -600_000 }, { name: 'Rahman' }), true, 'a name is not a BIN');
  assert.equal(theNoteWouldBeRefused({ vat_minor: -600_000 }, named), false);
});

test('under the figure, a note prints for whoever walked in', () => {
  assert.equal(theNoteWouldBeRefused({ vat_minor: -45_000 }, null), false);
  assert.equal(theNoteWouldBeRefused({ vat_minor: -500_000 }, null), false, 'exactly 5,000 is not more');
  assert.equal(theNoteWouldBeRefused({ vat_minor: -500_001 }, null), true);
});

test('the tax is read as a size, because a refund runs below nothing', () => {
  assert.equal(theNoteWouldBeRefused({ vat_minor: 600_000 }, null), true);
  assert.equal(theNoteWouldBeRefused({}, null), false, 'nothing taxed, nobody to name');
});
