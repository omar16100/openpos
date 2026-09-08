/// What to buy, from what sold and what is left.
///
/// The question a shopkeeper asks on the way to the wholesaler, and the one the
/// shop could not answer: it knew what had sold and it knew what was on the
/// shelf, and nothing put the two together. A list of what sold most is not
/// that answer, because the thing that sells most is usually the thing that is
/// still there.
///
/// Nothing here decides how much to buy. It says how many days the shelf lasts
/// at the rate the shop has been selling, and puts the shortest first: how much
/// to order depends on when the supplier comes, how much cash is in the drawer
/// and what the wholesaler has, none of which is in this machine.

/// A day, in milliseconds. Named because the arithmetic below reads as days.
const A_DAY = 86_400_000;

/// How long the shelf lasts, item by item, shortest first.
///
/// `sold` is what the shop sold over the window, as the report gives it:
/// `{ item, qty_milli }`. `onHand` is what is on the shelf now, by item id, as
/// the stock figures give it: `{ qty_milli }`. `windowMs` is the length of the
/// window those sales came from, which is what turns a quantity into a rate.
///
/// Items that sold nothing are left out. They are not running low; they are
/// either dead stock or something the shop does not really sell, and putting
/// them at the top of a buying list because they last forever would bury the
/// rice.
///
/// An item that sold and has nothing left is first, with zero days. An item the
/// shop holds no figure for reads as nothing left, which is what a shop that
/// has never counted holds as far as anything here knows: it is shown rather
/// than hidden, and the count is the fix.
export function daysOfStock(sold, onHand, windowMs) {
  const days = Math.max(windowMs, 1) / A_DAY;
  const rows = [];
  for (const row of sold ?? []) {
    const perDay = (row.qty_milli ?? 0) / Math.max(days, 1 / 24);
    if (perDay <= 0) continue;
    const left = onHand?.[row.item]?.qty_milli ?? 0;
    rows.push({
      item: row.item,
      // Negative means the shop's own figures say it has less than nothing,
      // which happens when goods left without a delivery behind them. Kept as
      // it is rather than clamped: a shelf that reads below zero is a thing to
      // look at, not a thing to round away.
      on_hand_milli: left,
      sold_milli: row.qty_milli ?? 0,
      per_day_milli: perDay,
      days_left: left <= 0 ? 0 : left / perDay,
    });
  }
  rows.sort((one, two) => one.days_left - two.days_left || one.item.localeCompare(two.item));
  return rows;
}

/// The ones worth walking to the wholesaler for: fewer days left than the shop
/// asked about.
export function runningLow(sold, onHand, windowMs, daysWanted) {
  return daysOfStock(sold, onHand, windowMs).filter((row) => row.days_left < daysWanted);
}
