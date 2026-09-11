/// What day it is where the shop is.
///
/// A shop's day is the one it trades in, not the one in Greenwich. Every date
/// box in the back office was filled in with `new Date().toISOString()`, which
/// is the UTC date, while the range those boxes describe is read as local
/// midnight to local midnight. East of Greenwich the two disagree for the first
/// hours of every morning, and Bangladesh is six hours east.
///
/// So a shopkeeper in Dhaka opening the trail at five in the morning was handed
/// a window that ended at midnight the night before: their own morning was
/// outside it and the screen said nothing had happened. Found exactly that way,
/// on a machine eight hours east, while trying to see whether a drawer opening
/// had reached the shop.
///
/// The day report is worse than empty. It defaulted to the UTC date too, so the
/// same shopkeeper at five in the morning read yesterday's takings under
/// today's heading, with nothing anywhere to say which day they were looking
/// at.
///
/// The figures themselves are unaffected: the request carries milliseconds
/// worked out from local midnight, which was always right. This is only about
/// which date the box starts on, which is the part somebody reads.

/// The date where this device is, as a form field wants it.
export function today(at = new Date()) {
  const year = at.getFullYear();
  const month = String(at.getMonth() + 1).padStart(2, '0');
  const day = String(at.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/// The same, some whole number of days ago.
///
/// Counted by setting the date rather than by subtracting milliseconds, so a
/// week before a clock change is still seven days and not seven days and an
/// hour. A shop that puts its clocks back would otherwise get a window that
/// quietly started an hour late once a year.
export function daysAgo(days, at = new Date()) {
  const then = new Date(at.getTime());
  then.setDate(then.getDate() - days);
  return today(then);
}

/// The month where this device is, for a return that covers one.
export function thisMonth(at = new Date()) {
  return today(at).slice(0, 7);
}
