/// Whether the price somebody typed is what the customer pays.
///
/// Two shops write a shelf label two different ways and both are ordinary. One
/// writes 480 and takes 480 at the counter, with the tax already inside it,
/// which is what a packaged price in this market usually is. The other writes
/// 480 as the price before tax and takes 552. The number is the same number.
/// Nothing in it says which shop this is, and the till charges a customer
/// fifteen percent apart on the two answers.
///
/// So it is asked, and until it is answered the item cannot be saved. It used
/// to be a checkbox that started unticked, which is an answer: an owner typing
/// the number off their own shelf and pressing Add priced that item above its
/// own label, on every sale of it, with nothing on any screen disagreeing.
///
/// A default was the wrong shape for this. A default is right where one answer
/// is ordinary and the other is unusual; here both are ordinary, the difference
/// is money at a counter, and the only thing that knows is the person typing.

/// Nothing said yet. What a form holds before somebody answers.
export const NOT_SAID = '';

/// The price is what the customer pays, tax and all.
export const TAX_IS_IN_IT = 'in';

/// The price is what the tax goes on top of.
export const TAX_COMES_ON_TOP = 'before';

/// Whether this answer can be saved.
///
/// Anything other than the two answers is unsaid, including a value from a
/// screen one build older or one build newer. There is no third meaning to fall
/// back to: a value nobody recognises read as "before tax" is the same fifteen
/// percent, reached a different way.
export function thePriceHasBeenExplained(answer) {
  return answer === TAX_IS_IN_IT || answer === TAX_COMES_ON_TOP;
}

/// What the shop's record calls it: `price_inclusive`, true or false.
///
/// Only ever asked of an answer that has been explained, so an unsaid one is
/// false here and the caller is the thing that must not have got this far. The
/// refusal is `thePriceHasBeenExplained` above, in one place, because two
/// places to ask is one place to forget.
export function theTaxIsInsideThePrice(answer) {
  return answer === TAX_IS_IN_IT;
}

/// And back the other way, for an item opened to be corrected.
///
/// An item already in the shop has been answered once, by whoever added it, and
/// a correction is not the moment to ask again: the form opens on what the item
/// says and an owner changing a name does not have to re-answer a question
/// about tax.
export function howThisItemWasPriced(item) {
  return item?.price_inclusive ? TAX_IS_IN_IT : TAX_COMES_ON_TOP;
}

/// The same question, asked of a file instead of a form.
///
/// A spreadsheet may answer it row by row, in a column this product reads, and
/// then there is nothing to ask. A file with no such column is asked once, for
/// the whole file, and only when it brings in a row the shop does not already
/// hold: an item already here keeps the answer it was given when somebody added
/// it, so a file that only moves prices on things the shop sells has nothing to
/// say about tax that the shop does not already know.
export function theFileMustSayWhatItsPricesAre(saidAboutTax, readyRows = []) {
  if (saidAboutTax) return false;
  return readyRows.some((row) => rowSaysNothing(row) && !row?.matched);
}

/// What one row of a file will be written as.
///
/// Three answers in order, and the order is the whole rule: what the row says,
/// then what the shop already holds for that item, then what somebody said
/// about the file. A row that says nothing about an item the shop has never
/// seen is the only one the file-level answer reaches.
export function howThisRowWillBePriced(row, held, fileAnswer) {
  if (!rowSaysNothing(row)) return Boolean(row.price_inclusive);
  if (held) return Boolean(held.price_inclusive);
  return theTaxIsInsideThePrice(fileAnswer);
}

/// A row whose file said nothing about tax. Null is what the reader writes.
function rowSaysNothing(row) {
  return row?.price_inclusive === null || row?.price_inclusive === undefined;
}
