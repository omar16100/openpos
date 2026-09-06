/// Money typed by a person, turned into the integers everything else here uses.
///
/// The rest of this product never sees a float on the money path: the core is
/// integer poisha from end to end. The screens are where that promise can still
/// be broken, because a text box hands back a string and `Number()` will take
/// "1e3" and "0.001" and give back something nobody typed.
///
/// So this accepts what a person can actually hand over: digits, optionally a
/// point and one or two more. Anything else is not an amount and is refused
/// where it was typed, rather than rounded quietly on the way to the server.
export function minorFrom(typed) {
  if (typeof typed !== 'string') return null;
  const trimmed = typed.trim();
  if (!/^\d{1,12}(\.\d{1,2})?$/.test(trimmed)) return null;
  const [whole, part = ''] = trimmed.split('.');
  return Number(whole) * 100 + Number(part.padEnd(2, '0'));
}
