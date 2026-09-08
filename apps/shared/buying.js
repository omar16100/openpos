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

/// The other end of the same question: what is sitting on the shelf not moving.
///
/// A small shop's cash is on its shelves. Something that has not sold in a
/// month is money the shop cannot spend on what does sell, and nothing in here
/// could say which things those were: the sold list shows what moved, and what
/// did not move is by definition not on it.
///
/// Valued at what the shop paid, not at what it hopes to sell for. What it
/// hopes for is not money it has, and the figure is being read to decide
/// whether to stop buying something.
///
/// `held` is what is on the shelf, by item id. `sold` is the same window's
/// sales. `costs` is what the shop pays for one, by item id, in poisha; an item
/// with no cost recorded is still listed, worth nothing that anybody can state,
/// because a shop should not be told its dead stock is smaller than it is.
export function notMoving(sold, held, costs) {
  const moved = new Set(
    (sold ?? []).filter((row) => (row.qty_milli ?? 0) > 0).map((row) => row.item),
  );
  const rows = [];
  for (const [item, figure] of Object.entries(held ?? {})) {
    const left = figure?.qty_milli ?? 0;
    if (left <= 0 || moved.has(item)) continue;
    const cost = costs?.[item] ?? 0;
    rows.push({
      item,
      on_hand_milli: left,
      // Poisha, from milli-units times poisha-per-unit. Rounded to the poisha
      // rather than carried as a fraction of one: this is money.
      worth_minor: Math.round((left * cost) / 1_000),
      costed: cost > 0,
    });
  }
  rows.sort(
    (one, two) => two.worth_minor - one.worth_minor || one.item.localeCompare(two.item),
  );
  return rows;
}
