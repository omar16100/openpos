/// Moving a lot of prices at once, which is what a shop does when the
/// wholesaler moves.
///
/// Prices here move together and often: a sack of rice goes up, and so does
/// every rice line on the shelf. Doing that one item at a time through a form
/// is an afternoon, and an afternoon nobody has, so the prices stay wrong and
/// the margin quietly goes.
///
/// Nothing here writes anything. It works out what each new price would be so
/// the owner can read the list before agreeing to it: a bulk change nobody
/// previewed is a shop that finds out at the till.

/// What one price becomes, rounded the way a shopkeeper quotes one.
///
/// To the nearest taka, because that is what goes on a shelf label and what
/// somebody says out loud. A price of 451.50 is not a price anybody in a shop
/// says, and leaving it there means the till hands back fifty poisha nobody
/// has.
///
/// Never below one taka: a rounding that lands on zero would put something on
/// the shelf for nothing.
export function movedPrice(priceMinor, percent) {
  const moved = priceMinor * (1 + percent / 100);
  const rounded = Math.round(moved / 100) * 100;
  return Math.max(rounded, 100);
}

/// What a run of items would become, for reading before agreeing.
///
/// Items with no price are left out: something the shop has never priced is not
/// something a percentage can move, and putting it in the list at zero invites
/// somebody to agree to a shelf full of one-taka goods.
export function repriced(items, percent) {
  if (!Number.isFinite(percent) || percent === 0) return [];
  return (items ?? [])
    .filter((item) => (item.price_minor ?? 0) > 0)
    .map((item) => ({
      id: item.id,
      name: item.name,
      was_minor: item.price_minor,
      now_minor: movedPrice(item.price_minor, percent),
    }))
    .filter((row) => row.now_minor !== row.was_minor);
}
