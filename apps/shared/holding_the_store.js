/// Waiting for the last window to let go, rather than guessing how long it takes.
///
/// A browser goes on holding a shop's ledger for a window that has already
/// gone. The tab is closed, nothing else is open, and every file still answers
/// that somebody else has it, so the screen ends up telling a shopkeeper to
/// switch the tablet off and on. Letting go on `pagehide` and trying again for
/// a second and a half fixed most of it and not all: the handover sometimes
/// takes longer than any number worth hard-coding, and a number that is too
/// short is the same failure with extra steps.
///
/// A web lock is the browser's own answer to this question. A lock is held by a
/// context, not by a file, and the browser releases it when that context dies,
/// whether it was closed, reloaded, crashed or killed for memory. So the page
/// arriving asks for the lock and waits: when it is granted, the last holder is
/// genuinely gone, and the files it held are free.
///
/// Bounded, because the other case is real. A till that is honestly open in
/// another window will never release, and a screen that waits for ever there is
/// worse than one that says so: after the wait, the shop gets the sentence it
/// already had, which is the right sentence for that case.

/// How long to wait for the last window to let go.
///
/// Long enough to cover a handover the browser is slow about, short enough that
/// a shopkeeper with the till genuinely open twice is told so rather than left
/// watching a blank screen. Five seconds is about the length of a shrug.
export const WAIT_FOR_MS = 5_000;

/// Take the lock for this terminal's store, or say it is somebody else's.
///
/// Answers with a function that gives it back. That function is safe to call
/// twice and does nothing the second time, because it is called both when a
/// page says it is leaving and when the store is opened again in the same
/// worker.
///
/// `locks` is the browser's lock manager, handed in so a test can be the
/// browser. A browser without one, which is any of them older than this
/// product, gets a lock that is granted immediately: it is no worse off than it
/// was before this existed.
export async function holdTheStore(terminal, { locks, waitMs = WAIT_FOR_MS } = {}) {
    const manager = locks ?? globalThis.navigator?.locks;
    if (!manager?.request) return () => {};

    const name = `openpos.store.${terminal}`;
    let release;
    const held = new Promise((settle) => {
        release = settle;
    });

    // Granted or refused, never left hanging. The request resolves when the
    // callback's promise settles, so the wait below is on being granted rather
    // than on the work that follows.
    let granted;
    const waitingToBeGranted = new Promise((settle) => {
        granted = settle;
    });

    const asked = manager
        .request(name, { mode: 'exclusive' }, () => {
            granted(true);
            return held;
        })
        .catch(() => {
            // The wait was given up on, below. Nothing to do about it here: the
            // caller is about to be told the store is open elsewhere.
            granted(false);
        });

    const gaveUp = new Promise((settle) => setTimeout(() => settle(false), waitMs));
    if (await Promise.race([waitingToBeGranted, gaveUp])) {
        return () => {
            release();
            // Not awaited. The page holding this is often on its way out, and
            // what matters is that the browser has been told.
            void asked;
        };
    }

    // Still somebody else's after the wait. Released if it is granted later, so
    // a lock nobody is using is not left held by a page that gave up on it.
    void waitingToBeGranted.then((got) => {
        if (got) release();
    });
    return null;
}
