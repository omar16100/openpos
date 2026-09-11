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
