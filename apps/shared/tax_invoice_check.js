/// Whether a sale can go on a Mushak 6.3 at all.
///
/// The form gives a line three figures and no more: the value excluding every
/// tax, the rate, and the tax in taka. Anybody reading it multiplies the first
/// two and expects the third, and an inspector doing that is the whole reason
/// the form has those columns in that order.
///
/// A sale can fail that arithmetic honestly. An item whose tax is fixed to its
/// listed price, which is a real regime and a setting on the item screen, keeps
/// its tax when a discount comes off: a hundred taka line with ten percent off
/// is charged for at ninety and taxed fifteen, because the discount comes out
/// of the shop's margin rather than off the tax. Both figures are right and
/// neither is a rate away from the other. The form has no column for the
/// hundred.
///
/// So the document is refused rather than printed wrong. A page an inspector
/// can disprove with a calculator is worse for the shop than no page: the
/// shopkeeper can give the customer the receipt and settle it with their
/// accountant, and what they cannot do is unprint a form.
///
/// This is a check rather than a calculation, and the difference matters here.
/// Nothing on a screen works out what tax is due: the crate that priced the
/// sale did that, and both figures come from it. What happens here is that two
/// numbers the shop has already declared are read against the rate it has
/// already declared, and the answer is only ever used to refuse.

/// How far apart the two may be and still agree: one poisha.
///
/// Rounding a rate onto an amount lands on a half often enough that an exact
/// comparison would refuse honest lines, and a poisha is smaller than any
/// difference that means anything. The case this exists for is out by a hundred
/// and fifty of them.
const NEAR_ENOUGH = 1;

/// The lines whose declared tax is not the rate times what was charged.
///
/// Empty for an ordinary sale, which is nearly all of them.
export function linesTheFormCannotCarry(lines = []) {
  return lines.filter((line) => {
    const net = Number(line?.net_minor ?? 0);
    const vat = Number(line?.vat_minor ?? 0);
    const rate = Number(line?.vat_bp ?? 0);
    if (!Number.isFinite(net) || !Number.isFinite(vat) || !Number.isFinite(rate)) return false;
    // Rounded away from zero on a half, which is how this product rounds money
    // everywhere: a refund is the mirror of its sale and has to come back to
    // the same figure.
    const expected = Math.sign(net * rate) * Math.round(Math.abs((net * rate) / 10_000));
    return Math.abs(vat - expected) > NEAR_ENOUGH;
  });
}

/// Whether this sale can be put on the form at all.
export function theFormCanCarry(lines = []) {
  return linesTheFormCannotCarry(lines).length === 0;
}

/// Whether these goods went out of the shop or came back into it.
///
/// The Mushak 6.3 is the paper for a supply. Goods coming back are not one:
/// they are a decreasing adjustment, and section 52 of the Act prescribes a
/// credit note for them, carrying things this form has no place for, among them
/// the serial number and time of the invoice being adjusted.
///
/// The till offered the form for a refund, and it laid one out: a কর চালানপত্র
/// with a quantity of -1 and a total below nothing, walked on a live till. A
/// business buyer handed that has the wrong document for their return, and it
/// is headed as the right one.
///
/// Read off the money rather than off a flag, because every screen that can
/// reach this has the money and only some of them have the flag: a refund
/// against a receipt nobody could produce carries no original number to test.
export function goodsCameBack(sale) {
  const total = Number(sale?.total_minor ?? sale?.totalMinor ?? 0);
  return Number.isFinite(total) && total < 0;
}

/// Whether a credit note may be printed for these goods coming back.
///
/// Section 52(1)(f) makes the buyer's name, address and BIN part of the note
/// once the VAT on the supply is more than 5,000 taka, and 52(2) says a note
/// without them "shall not be used in support of a claim for any decreasing
/// adjustment". So a note that would be refused by the rule it exists for is
/// not printed: the buyer is picked first, and the screen says so.
///
/// Under that figure the note prints with or without them, which is the same
/// answer the Act gives: the clause is conditional, and a shop handing back
/// four hundred taka of tax to somebody who walked in off the street has
/// nobody to name.
export function theNoteWouldBeRefused(sale, buyer) {
  const vat = Math.abs(Number(sale?.vat_minor ?? 0));
  if (!Number.isFinite(vat) || vat <= NAME_THE_BUYER_ON_A_CREDIT_ABOVE) return false;
  return !buyer?.name || !buyer?.bin;
}

/// Five thousand taka of VAT, in poisha. Section 52(1)(f).
///
/// The same figure the core holds as `NAME_THE_BUYER_ON_A_CREDIT_ABOVE`, and
/// the till reads the core's answer through the view rather than this: what
/// this file decides is narrower, whether the document may be printed at all,
/// and it is decided here because both screens print it and only one of them
/// has a view.
const NAME_THE_BUYER_ON_A_CREDIT_ABOVE = 500_000;
