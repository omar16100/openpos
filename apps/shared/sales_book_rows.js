/// Turning a month of shelf movements into the rows form মূসক-৬.২ is read in.
///
/// The form is a page per product with a row per day: what was on the shelf,
/// what came in with the supplier's own invoice number beside it, what went
/// out, and what is left. The shop answers with movements and their moments,
/// because which day a movement belongs to depends on where the clock is: two
/// in the morning in Dhaka is the previous evening in UTC, and the day a shop
/// keeps is the one its own devices keep. So the grouping happens here, on the
/// screen, the way every other day in this product is decided.
///
/// A day with two deliveries gets two rows, because the form gives one row four
/// columns for one purchase and a shop that took two lorries in a morning has
/// two invoice numbers to show. The sales and the closing balance sit on the
/// first row of the day, where a person reading the page down the closing
/// column sees one figure per day.

import { today } from './days.js';

/// What moved a shelf, by the shop's own numbering.
const A_SALE = 1;
const GOODS_ARRIVING = 2;

/// One month of movements as the form's rows, oldest day first.
///
/// `opening` is not needed here: the running balance is worked out by the
/// screen that draws the page, because it is the one place that knows what the
/// page opened at.
export function theBookByDay(moved = []) {
  const days = new Map();
  for (const one of moved) {
    const day = today(new Date(Number(one?.at_ms) || 0));
    if (!days.has(day)) {
      days.set(day, { day, cameIn: [], soldMilli: 0, correctedMilli: 0 });
    }
    const row = days.get(day);
    const qty = Number(one?.qty_milli) || 0;
    if (one?.kind === GOODS_ARRIVING) {
      row.cameIn.push({
        reference: one.reference ?? '',
        supplierName: one.supplier_name ?? '',
        supplierBin: one.supplier_bin ?? '',
        qtyMilli: Math.abs(qty),
        atMs: Number(one?.at_ms) || 0,
      });
    } else if (one?.kind === A_SALE) {
      // Goods going out, as a size: the form's বিক্রয় column counts what left
      // the shelf, and a return that day reduces it rather than appearing as a
      // delivery from nobody.
      row.soldMilli -= qty;
    } else {
      // A write-off or a count correction. The form has no column for it, and a
      // page that left it out would stop adding up, which is the one thing a
      // book must not do.
      row.correctedMilli += qty;
    }
  }
  return [...days.values()].sort((a, b) => (a.day < b.day ? -1 : a.day > b.day ? 1 : 0));
}

/// The same rows with the balances filled in, which is what the page shows.
///
/// Four figures a row, and the form names three of them: মোট is what the shelf
/// held plus what came in, বিক্রয় is what went out, and সমাপনী জের is the first
/// less the second. The fourth is everything else that moved it, which the form
/// has no column for and which is carried into the balance all the same.
export function theBookWithBalances(openingMilli, rows = []) {
  let running = Number(openingMilli) || 0;
  return rows.map((row) => {
    const cameInMilli = row.cameIn.reduce((total, one) => total + one.qtyMilli, 0);
    const opening = running;
    const total = opening + cameInMilli;
    const closing = total - row.soldMilli + row.correctedMilli;
    running = closing;
    return { ...row, openingMilli: opening, cameInMilli, totalMilli: total, closingMilli: closing };
  });
}
