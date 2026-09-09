/// Why the till's own files would not open, in words a shop can act on.
///
/// The browser's own sentence for the commonest of these is "Failed to execute
/// 'createSyncAccessHandle' on 'FileSystemFileHandle': Access Handles cannot be
/// created if there is another open Access Handle or Writable stream associated
/// with the same file." That is true, English, and useless to the person
/// standing at the counter, and it appeared on a real screen: the till showed
/// that line and, underneath it, the box for an enrolment code.
///
/// The second part is the dangerous one. The till had not lost anything: the
/// same shop's ledger was open in another tab a swipe away. A shopkeeper who
/// followed the screen would have enrolled the device again, which mints a
/// second terminal with its own block of receipt numbers while the sales, the
/// parked baskets and the numbers already handed out stay in the tab nobody is
/// looking at. The screen invited the one action that loses something.
///
/// So the reason is named here, once, as a plain function over whatever the
/// browser threw. Nothing in this file touches a browser, which is what makes
/// it testable: the failures below are the exact shapes Chrome and Safari
/// produce, written down.

/// Every reason this file can name. Exported so the dictionary can be held to
/// it: a code with no words is a screen showing a code to a shopkeeper.
export const EVERY_STORAGE_TROUBLE = [
  'till-open-elsewhere',
  'no-room-on-this-device',
  'this-browser-keeps-nothing',
];

/// Name what went wrong opening the files, or `null` if this is not one we
/// know.
///
/// `null` rather than a guess. A failure nobody has seen before keeps the
/// browser's own sentence, which is worth more to whoever is sent to look at it
/// than a wrong name would be.
export function whyStorageFailed(trouble) {
  const named = trouble?.name ?? '';
  const said = String(trouble?.message ?? trouble ?? '');

  // Chrome throws NoModificationAllowedError when another handle holds the
  // file; Safari has used InvalidStateError for the same thing. Matched on the
  // name first because it is the part that is specified, and on the sentence
  // second because the name has already differed between two browsers and the
  // cost of being wrong here is a shopkeeper enrolling a till that was fine.
  if (named === 'NoModificationAllowedError' || named === 'InvalidStateError') {
    return 'till-open-elsewhere';
  }
  if (/access handle|another open access handle|writable stream/i.test(said)) {
    return 'till-open-elsewhere';
  }

  // The device is full. A till that cannot write cannot sell, and what a shop
  // does about it is delete something, not re-enrol.
  if (named === 'QuotaExceededError' || /quota|storage full|no space/i.test(said)) {
    return 'no-room-on-this-device';
  }

  // A browser with storage switched off, or a private window that has none.
  // The till still sells in memory, and the shop has to know that nothing
  // survives closing the tab.
  if (named === 'SecurityError' || /storage.*(denied|not allowed|unavailable)/i.test(said)) {
    return 'this-browser-keeps-nothing';
  }

  return null;
}

/// The same failure, in the shape the rest of the app carries failures in.
///
/// A new Error rather than the browser's own object, and that is not tidiness.
/// What the browser throws here is a `DOMException`, whose `code` is a
/// read-only getter left over from an older standard: assigning to it inside a
/// module throws a TypeError, so the code that was meant to reach the screen
/// would instead become a second failure thrown from the handler for the first
/// one. The original travels as the cause for whoever is sent to look.
export function storageTrouble(trouble) {
  const passed = new Error(String(trouble?.message ?? trouble ?? 'the till could not open its files'), {
    cause: trouble,
  });
  passed.code = whyStorageFailed(trouble);
  return passed;
}

/// Whether this is the till simply being open somewhere else on this device.
///
/// Its own question because one screen decision hangs on it: a till that is
/// open elsewhere must not be offered an enrolment box. Everything else about a
/// failed open leaves that offer where it was, because a device that genuinely
/// has no ledger is a device somebody does have to enrol.
export function alreadyOpenHere(code) {
  return code === 'till-open-elsewhere';
}
