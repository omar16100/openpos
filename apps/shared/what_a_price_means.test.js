import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  NOT_SAID,
  TAX_COMES_ON_TOP,
  TAX_IS_IN_IT,
  howThisItemWasPriced,
  howThisRowWillBePriced,
  theFileMustSayWhatItsPricesAre,
  thePriceHasBeenExplained,
  theTaxIsInsideThePrice,
} from './what_a_price_means.js';

test('a price nobody has explained cannot be saved', () => {
  assert.equal(thePriceHasBeenExplained(NOT_SAID), false);
  assert.equal(thePriceHasBeenExplained(undefined), false);
  assert.equal(thePriceHasBeenExplained(null), false);
});

test('both answers are answers', () => {
  assert.equal(thePriceHasBeenExplained(TAX_IS_IN_IT), true);
  assert.equal(thePriceHasBeenExplained(TAX_COMES_ON_TOP), true);
});

test('nothing else is read as an answer', () => {
  // The case this exists for: a value that is not one of the two must not fall
  // through to "before tax", which is what a boolean did. 480 charged at 552 is
  // the same mistake whether it was reached by a default or by a typo.
  for (const wrong of ['inclusive', 'IN', 'true', true, false, 0, 1, {}]) {
    assert.equal(thePriceHasBeenExplained(wrong), false, `${String(wrong)} is not an answer`);
  }
});

test('the two answers mean what the shop records', () => {
  assert.equal(theTaxIsInsideThePrice(TAX_IS_IN_IT), true);
  assert.equal(theTaxIsInsideThePrice(TAX_COMES_ON_TOP), false);
});

test('an item opened for correction answers for itself', () => {
  assert.equal(howThisItemWasPriced({ price_inclusive: true }), TAX_IS_IN_IT);
  assert.equal(howThisItemWasPriced({ price_inclusive: false }), TAX_COMES_ON_TOP);
  // Both round trip, which is what makes a correction that changes a name leave
  // the price alone.
  for (const was of [true, false]) {
    assert.equal(theTaxIsInsideThePrice(howThisItemWasPriced({ price_inclusive: was })), was);
  }
});

test('an item from a shop that has never said reads as before tax, and is answered', () => {
  // Nothing in the shop is unanswered: `price_inclusive` is a boolean on the
  // wire and every item carries one. What is missing here is the item itself,
  // which is a form opened on nothing.
  assert.equal(howThisItemWasPriced(undefined), TAX_COMES_ON_TOP);
  assert.equal(thePriceHasBeenExplained(howThisItemWasPriced(undefined)), true);
});

/// A row as the file reader hands it over: null where the file said nothing.
const row = (price_inclusive, matched = null) => ({ price_inclusive, matched });

test('a file with its own column is not asked anything', () => {
  assert.equal(theFileMustSayWhatItsPricesAre(true, [row(null), row(true)]), false);
});

test('a file that says nothing is asked, when it brings in something new', () => {
  assert.equal(theFileMustSayWhatItsPricesAre(false, [row(null)]), true);
});

test('a file that only corrects what the shop already holds is not asked', () => {
  // Every row matches an item, and every one of those items was answered when
  // somebody added it. Asking again would be asking a shopkeeper changing
  // prices to restate something they cannot get wrong by leaving alone.
  assert.equal(
    theFileMustSayWhatItsPricesAre(false, [row(null, { id: 'a' }), row(null, { id: 'b' })]),
    false,
  );
});

test('one new row among a hundred corrections is enough to be asked', () => {
  const rows = [...Array(100)].map((_, at) => row(null, { id: `held-${at}` }));
  rows.push(row(null));
  assert.equal(theFileMustSayWhatItsPricesAre(false, rows), true);
});

test('what a row is written as, in order', () => {
  // The row's own answer wins.
  assert.equal(howThisRowWillBePriced(row(true), { price_inclusive: false }, TAX_COMES_ON_TOP), true);
  assert.equal(howThisRowWillBePriced(row(false), { price_inclusive: true }, TAX_IS_IN_IT), false);
  // Then the item the shop holds.
  assert.equal(howThisRowWillBePriced(row(null), { price_inclusive: true }, TAX_COMES_ON_TOP), true);
  // Then the answer given for the file.
  assert.equal(howThisRowWillBePriced(row(null), null, TAX_IS_IN_IT), true);
  assert.equal(howThisRowWillBePriced(row(null), null, TAX_COMES_ON_TOP), false);
  // And an unanswered file is exclusive here, which is only ever reached by a
  // caller that skipped the refusal above.
  assert.equal(howThisRowWillBePriced(row(null), null, NOT_SAID), false);
});
