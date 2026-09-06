// Saving a record that may be new or may be a correction.
//
// This app got the same thing wrong twice in three days: the catalogue form and
// the supplier form both minted an id on every save, so correcting a price or a
// phone number wrote a second record beside the first instead of changing it.
// The second form had the bug because it was written by copying the first,
// before the first was fixed.
//
// Both mistakes are the same shape and neither is visible in a diff: an id
// arrives from the right place or the wrong one, and the screen looks identical
// either way. So the decision lives here, once, with tests, rather than in each
// form where it can be forgotten again.

/// What a save is addressed to, and what it must not quietly change.
///
/// `correcting` is the record being corrected, or null when this is new.
/// `mint` makes an id, called only when one is needed: an id minted and thrown
/// away is harmless, but calling it unconditionally is exactly the bug.
/// `carry` names the fields a correction has to preserve, with the value a new
/// record should get.
///
/// Returns the id and the carried fields together, because they are wrong in
/// the same way for the same reason: a default sent during a correction undoes
/// something nobody edited. `active` puts a withdrawn item back on the shelf. A
/// zero cost wipes a margin the shop cannot recover.
export function saving(correcting, mint, carry = {}) {
  const kept = {};
  for (const [field, fallback] of Object.entries(carry)) {
    const held = correcting?.[field];
    kept[field] = held === undefined || held === null ? fallback : held;
  }
  return { id: correcting?.id ?? mint(), ...kept };
}
