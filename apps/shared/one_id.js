/// The id a not-yet-recorded thing is sent under.
///
/// The shop deduplicates on the id it is sent, so a press repeated after a
/// reply went missing has to carry the same one: minting a fresh id at every
/// press is how a basket becomes two sales, a delivery becomes two deliveries,
/// and what the shop owes its supplier is counted twice.
///
/// Keeping it for ever is the other half of the same mistake. A booking whose
/// reply was lost leaves the form open; if the owner then changes what is on it
/// and presses again, the shop sees the id it already has, calls the press a
/// repeat, and drops what was typed. Worse for a person: the id is upserted, so
/// adding Amina, losing the reply, and typing Rahima over the same form renames
/// Amina rather than adding anybody.
///
/// So the id belongs to what is on the form. Press again with the same thing
/// and it is the same id, which is the repeat the shop should ignore. Change
/// anything and it is a new thing, with a new id.
export function idForThisOne(kept, shape, mint) {
  if (kept && kept.shape === shape) return kept;
  return { id: mint(), shape };
}

/// What is on the form, as one string to compare.
///
/// `JSON.stringify` of the values in the order given: the caller decides what
/// counts, because what makes a delivery a different delivery is its lines and
/// its supplier, and what makes a person a different person is their name.
export function whatIsOnTheForm(...parts) {
  return JSON.stringify(parts);
}
