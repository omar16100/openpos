/// Opening a till's own files when the last window has not finished letting go.
///
/// A browser can go on holding a shop's ledger for a window that has already
/// gone. Reload the till and the files are still answering that somebody else
/// has them: the tab that had them is closed, nothing else is open, and the
/// only advice the screen could give was to switch the device off and on. That
/// is a real answer to give a shopkeeper at a counter about once, and it was
/// given three times in one afternoon of walking.
///
/// What is actually happening is a handover, not a conflict. The page that held
/// the files is being torn down while the page that wants them is already
/// asking, and the gap is short: the old context goes, its handles are
/// released, and the same open succeeds. So the fix is to wait through the
/// handover rather than to report it.
///
/// Only for the one reason, and only for a moment. A till that is genuinely
/// open in another window is a different thing with a different answer, and the
/// screen already says it well: a shopkeeper must not be made to wait ten
/// seconds to read it. Four tries over about a second and a half is invisible
/// when it works and costs nothing worth counting when it does not.

import { alreadyOpenHere } from './storage_trouble.js';

/// How long to wait before each try after the first, in milliseconds.
///
/// Short then longer, because the common case is a page already on its way out
/// and the first gap catches most of it. The total is the sum of these: about a
/// second and a half, which is under what a person reads as a pause.
export const WAITS = [120, 350, 1000];

/// Open the files, waiting through a handover that has not finished.
///
/// `attempt` is whatever actually opens them and is called again unchanged.
/// `pause` is how to wait, handed in so a test does not spend real seconds.
///
/// Everything other than "somebody else has these" is thrown at once: a device
/// with no room and a browser that keeps nothing are not going to change their
/// minds in a second, and a shop reading either of those should read it now.
export async function openingAgain(attempt, { waits = WAITS, pause = sleep } = {}) {
  let last;
  for (let turn = 0; turn <= waits.length; turn += 1) {
    try {
      return await attempt();
    } catch (trouble) {
      last = trouble;
      if (!alreadyOpenHere(trouble?.code)) throw trouble;
      // The last turn has no wait after it: the refusal that stands is the one
      // the screen shows, and it is the same refusal it always showed.
      if (turn === waits.length) break;
      await pause(waits[turn]);
    }
  }
  throw last;
}

function sleep(ms) {
  return new Promise((settle) => setTimeout(settle, ms));
}
