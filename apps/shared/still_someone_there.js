/// A worker that has lost its page lets the shop's ledger go.
///
/// The failure this is against was found with every window closed. Three tills
/// were still syncing to the shop, still holding their stores, and the shop's
/// device list still said they had been reached seconds ago. Nobody was at any
/// of them: the tabs were shut. A dedicated worker is supposed to go when the
/// page that made it goes, and these did not, so what was left behind held the
/// files, answered every later window that somebody else had them, and told the
/// shop a counter was live. The screen's advice was to close the other window.
/// There was no other window. Only restarting the browser cleared it.
///
/// The page says it is leaving on `pagehide`, and that is still the fast path,
/// but it is a message posted by something that is already going and it does
/// not always arrive. So the worker also asks, and lets go when nothing answers.
///
/// Asked by the worker rather than announced by the page, on purpose. A page's
/// timers are throttled to about once a minute when its tab is not in front and
/// can stop altogether, which is the whole reason the sync loop was moved in
/// here; a heartbeat sent from the screen would look exactly like a page that
/// had gone. Answering a question is not throttled that way, so a hidden page
/// answers at once and a page that is frozen or gone answers never. That is the
/// difference this needs to see, and nothing else tells the two apart.

/// How often the worker asks its page whether it is still there.
export const ASK_EVERY_MS = 5_000;

/// How many asks may go unanswered before the files are let go.
///
/// Four asks, so about twenty seconds of silence. Long enough that a screen
/// busy with a long task is not mistaken for a screen that has gone, short
/// enough that somebody who closed a window and opened another is not left
/// waiting at the counter.
export const PATIENCE = 3;

/// Ask, and let go when nothing answers.
///
/// `ask` posts the question to the page. `letGo` closes the files and gives the
/// lock back. `timers` is handed in so a test can run twenty seconds in no time
/// at all.
export function keepAskingWhoIsThere({
    ask,
    letGo,
    everyMs = ASK_EVERY_MS,
    patience = PATIENCE,
    timers = globalThis,
} = {}) {
    let unanswered = 0;
    let stopped = false;

    const round = timers.setInterval(() => {
        unanswered += 1;
        if (unanswered > patience) {
            stop();
            letGo();
            return;
        }
        ask();
    }, everyMs);

    function stop() {
        if (stopped) return;
        stopped = true;
        timers.clearInterval(round);
    }

    return {
        /// The page answered. Whatever it was doing before, it is there now.
        answered() {
            unanswered = 0;
        },
        /// Nothing to watch any more, because the files have been let go for a
        /// reason of their own.
        stop,
        /// For the test, and for a log line worth having when a till drops its
        /// files without being told to.
        unanswered: () => unanswered,
    };
}
