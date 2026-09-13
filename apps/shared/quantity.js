/// What a typed quantity means.
///
/// Its own module because both screens type them: the shelf being counted in
/// the back office, and the loose rice being weighed at the till. One rule, one
/// place, tested once. A second parser would be a second answer to "what is
/// 1.5", and the two would differ on the day somebody typed 1.5005.

/// Quantities as anybody types them, in thousandths.
///
/// Digits, optionally a point and up to three more, because a shelf can hold
/// 1.5 kg and cannot hold 1.5005 of anything this shop sells. Refused rather
/// than rounded: a count is the number that replaces the running figure, and a
/// quantity nobody typed is the worst possible thing to put there.
export function milliFrom(typed) {
  if (typeof typed !== 'string') return null;
  const trimmed = typed.trim();
  if (!/^\d{1,9}(\.\d{1,3})?$/.test(trimmed)) return null;
  const [whole, part = ''] = trimmed.split('.');
  return Number(whole) * 1000 + Number(part.padEnd(3, '0'));
}

/// How many are on a line, whichever way the ticket runs.
///
/// A refund's quantities are kept below nothing, because that is what makes the
/// arithmetic of a return come out as the mirror of the sale it undoes. The box
/// a cashier types into is not about that: it asks how many, and how many is
/// three whether they are going out of the shop or coming back into it.
export function howManyOnTheLine(qtyMilli) {
  const milli = Number(qtyMilli);
  return Number.isFinite(milli) ? Math.abs(milli) : 0;
}

/// What to send when somebody asks for this many on a ticket running this way.
///
/// The sign belongs here and nowhere else. It was nowhere, which cost a cashier
/// the ability to take back more than one of anything by typing: the box showed
/// `-1`, typing `-1` was refused as not a quantity, because a sign in a
/// quantity box is what wrote a thousand off a shelf, and typing `1` was
/// refused by the core as a sale and a return in one ticket. What was left was
/// pressing a button once per unit, on a screen where the button that reads
/// like "one more" was the one that took the line off.
export function askedForOnThisTicket(howManyMilli, comingBack) {
  const many = howManyOnTheLine(howManyMilli);
  return comingBack ? -many : many;
}
