/// Which commands need a till already open, and which do not.
///
/// The worker holds one till and opens it when the app says who this device is.
/// Everything else it can be asked to do arrives through the same door, and the
/// door used to refuse anything that arrived before that: a device enrolling
/// for the first time asked for the sync loop as it booted, was told the till
/// was not open yet, and never asked again. It enrolled, said "nobody has been
/// added to this shop yet", and stayed that way until somebody reloaded the
/// page. On the next boot the open comes first and it works, which is why this
/// only ever bit the first device of a shop and the first minutes of a new one.
///
/// A rule in one place, because the answer is a property of the command rather
/// than of the order the worker's file happens to be written in.

/// Commands that mean something before this device knows who it is.
///
/// `connect` says where the shop is, `open` is what opens the till, `enrol`
/// turns a code into a credential and has to run before any till exists at all,
/// `mark` reads a pasted bundle and hashes it, and `sync_loop` only arms a
/// timer whose every round waits for a till of its own accord.
const BEFORE_A_TILL = new Set(['connect', 'open', 'enrol', 'mark', 'sync_loop']);

/// Whether this command has to wait for a till to be open.
export function needsAnOpenTill(kind) {
  return !BEFORE_A_TILL.has(kind);
}
