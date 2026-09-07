// Telling two people with the same name apart.
//
// A shop can have two Karims. Nothing on the record distinguishes them except
// the id, so two identical entries appear wherever people are listed.
//
// For the people who may sign in, pressing the wrong one hands a whole shift to
// somebody who was not standing there: every sale, every drawer opening and
// every waiver. For the people who buy on account it is worse, because it is
// money: what one of them took goes on one account and what they paid goes on
// the other, and neither balance belongs to anybody.
//
// The same three functions serve both, because the records are the same shape
// and the mistake is the same mistake.
//
// This is for looking at, not for keying anything. Money is keyed by the core,
// which folds names its own way and is tested for it. The two must not be
// confused: this one only decides what a button says.

/// A name as two people saying it out loud would compare it.
export function fold(name) {
  return String(name ?? '')
    .trim()
    .toLowerCase()
    .replace(/\s+/g, ' ');
}

/// The folded names held by more than one person who can sign in.
///
/// Only the active ones. Somebody retired last year is not standing at the
/// counter, and marking a live person because of them would put identifiers on
/// a screen for no reason.
export function shared(people) {
  const seen = new Map();
  for (const person of people ?? []) {
    if (person?.active === false) continue;
    const name = fold(person?.name);
    seen.set(name, (seen.get(name) ?? 0) + 1);
  }
  const twice = new Set();
  for (const [name, count] of seen) {
    if (count > 1) twice.add(name);
  }
  return twice;
}

/// What a button should say for this person.
///
/// Their name, and only where it would otherwise be ambiguous, the tail of
/// their id: an arbitrary handle, but a stable one that the back office shows
/// beside the same person, so an owner can say "you are the Karim ending 7QF3".
/// A shop with one Karim never sees it.
export function label(person, twice) {
  const name = person?.name ?? '';
  if (!twice?.has(fold(name))) return name;
  const id = String(person?.id ?? '');
  return id.length > 4 ? `${name} · ${id.slice(-4)}` : name;
}

/// Whether somebody who can sign in already holds this name.
///
/// Used to say so before a second one is added, rather than to refuse: a shop
/// can have two Karims, and the answer is a name that tells them apart, not a
/// form that will not save.
export function nameTaken(people, name, exceptId = null) {
  const wanted = fold(name);
  return (people ?? []).some(
    (person) =>
      person?.active !== false &&
      person?.id !== exceptId &&
      fold(person?.name) === wanted,
  );
}
