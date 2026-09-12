/// What a PIN may be.
///
/// Its own module because the rule is a promise made on one screen and kept on
/// another: the back office is where a PIN is set, and the counter is where it
/// has to be typed. Those are different devices, often in different rooms, and
/// the person who finds out they disagree is a cashier at the start of a shift
/// with a queue in front of them.
///
/// Digits, four or more. The length was the only thing checked, and the box
/// said "PIN, four digits or more" while accepting "abcd": the screen then
/// said "Walk Letter Pin can sign in once the tills refresh", which was not
/// true of any tablet in the shop. The PIN box at a till is `inputmode`
/// numeric, so the keyboard that comes up on a phone or a tablet is a number
/// pad, and a PIN with a letter in it is one that cannot be typed on the device
/// it is for. The owner setting it is usually at a desk with a full keyboard,
/// which is exactly why nobody notices.
///
/// Nothing is trimmed. A space is not a digit, and accepting " 1234" here would
/// set a PIN whose first character a cashier cannot see and would never think
/// to type.
///
/// No rule about which digits. A shop that wants to use 1111 is a shop that has
/// weighed being locked out against being robbed and decided, and this is not
/// the place to overrule them; what this refuses is a PIN that cannot be
/// entered at all.

/// The fewest digits a PIN may have.
export const LEAST_DIGITS = 4;

/// The PIN as typed, or null when it is not one.
export function pinFrom(typed) {
  if (typeof typed !== 'string') return null;
  return new RegExp(`^\\d{${LEAST_DIGITS},}$`).test(typed) ? typed : null;
}
