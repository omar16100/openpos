/// Asking the window that has the shop to let go of it.
///
/// One device may have the ledger open in one window, which is the rule the
/// store lock keeps and is not in question here: two windows writing one ledger
/// is corruption, not a thing to be clever about.
///
/// What is in question is what the second window can do about it. Until this it
/// could say "close the other one", and on a tablet that is not always an
/// instruction somebody can follow: the other window is a tab behind this one,
/// or in a browser the shopkeeper did not know was still open, and the advice
/// under it is to switch the device off and on. In a shop with a queue that is
/// not advice, it is an outage.
///
/// So the window that has it is asked. It is alive, it is running this same
/// code, and letting go is the thing it already does when its page is closed:
/// it closes the files and releases the lock. Then the window somebody is
/// standing at opens the shop, and the one nobody is standing at says where it
/// went and offers to take it back.
///
/// The arriving window wins. That is the decision, and it is made here rather
/// than left to whichever browser is faster: somebody is standing in front of
/// the window that is asking, and by definition nobody is standing in front of
/// the other one, or they would not have opened this. The exception is the one
/// that matters more than the rule, and it is the holder's to make.
///
/// The holder refuses while it is in the middle of something a person would
/// lose. A basket rung and not paid for, money half counted into a drawer: the
/// window that has the shop is the only one that knows, and it says so rather
/// than dropping it. The asking window is told, in those words, and the person
/// can go and find that window or wait.
///
/// Nothing here decides on its own. Both sides are a press: the shopkeeper at
/// the new window asks, and the old window answers by rule. A window that let
/// go does not take the shop back by itself, because two windows that both
/// reach for it is the ping-pong this is meant to end.

/// How long to wait for an answer before saying nobody was there.
///
/// A broadcast is delivered in the same tick to a window that is awake. This is
/// long enough for one that was asleep in a background tab and short enough
/// that a cashier is not watching a spinner: the answer, either way, is one
/// sentence and a button.
export const WAIT_FOR_AN_ANSWER_MS = 2_000;

/// Which window this is.
///
/// A broadcast channel does not deliver a message back to the object that
/// posted it, and that is not the same as not delivering it back to the window:
/// the side that asks and the side that answers each hold their own channel, so
/// this window's answering side hears this window's asking. It did. The window
/// that pressed the button answered itself, gave up a store it did not have,
/// and told the person the shop had moved to another window, which was this one.
///
/// Made once for the life of the page, so both sides of this module agree about
/// who they are. Both take it as an argument so that a test can be two windows
/// in one process, which is what a test of this has to be.
export const THIS_WINDOW = `${Date.now()}.${Math.random()}`;

/// The channel one device's windows talk on, per store.
///
/// Named after the terminal rather than the shop, because two different tills
/// on one device are two different stores and neither has anything to say to
/// the other.
export function channelFor(terminal, { make } = {}) {
  const open = make ?? ((name) => new BroadcastChannel(name));
  return open(`openpos.store.${terminal}`);
}

/// Ask whoever has this store to let go of it.
///
/// Answers `{ said }` where `said` is `'let_go'` when somebody had it and gave
/// it up, `'busy'` when somebody had it and is in the middle of something, and
/// `'nobody'` when nothing answered, which is the ordinary case for a store
/// held by a window that has already gone.
///
/// A refusal carries `because`: what the other window is in the middle of, in
/// its own words. Without it the screen has one sentence for every refusal, and
/// the first one written said "the other window has a sale in progress" on the
/// back office, which rings no sales. The window that refuses is the only one
/// that knows what it is doing, so it is the one that says.
export async function askForTheStore(
  terminal,
  { make, waitMs = WAIT_FOR_AN_ANSWER_MS, timers = globalThis, from = THIS_WINDOW } = {},
) {
  const channel = channelFor(terminal, { make });
  const asked = `${Date.now()}.${Math.random()}`;

  return await new Promise((answer) => {
    const done = (said) => {
      channel.onmessage = null;
      try {
        channel.close();
      } catch {
        // A channel already closed is the state we want.
      }
      answer(said);
    };

    channel.onmessage = (event) => {
      // Only the answer to this asking. A window that asked a moment ago and
      // gave up must not take this one's answer.
      if (event.data?.answering !== asked) return;
      done({ said: event.data.let_go ? 'let_go' : 'busy', because: event.data.because ?? null });
    };
    timers.setTimeout(() => done({ said: 'nobody', because: null }), waitMs);
    channel.postMessage({ asking: asked, from });
  });
}

/// Answer windows that ask for the store this one is holding.
///
/// `busy` says what this window is in the middle of, or nothing when it is in
/// the middle of nothing: a word the screen on the other end turns into a
/// sentence, because only this window knows whether that is a sale or a count.
/// It is asked at the moment somebody asks rather than kept up to date, because
/// what a cashier has half done changes with every scan. `letGo` gives
/// the store up and is awaited, so the answer goes back only once the files are
/// actually free and the window that asked will find them so.
///
/// Answers with a function that stops listening, for a window on its way out.
export function answerWhoAsks(terminal, { busy, letGo, make, me = THIS_WINDOW } = {}) {
    const channel = channelFor(terminal, { make });
    channel.onmessage = async (event) => {
        const asked = event.data?.asking;
        if (!asked) return;
        // Not this window's own asking. See `THIS_WINDOW`.
        if (event.data.from === me) return;
        // Truthy is busy, and what it says is what this window is in the
        // middle of: the screen on the other end turns it into a sentence.
        const doing = busy?.();
        if (doing) {
            channel.postMessage({ answering: asked, let_go: false, because: doing });
            return;
        }
        await letGo?.();
        channel.postMessage({ answering: asked, let_go: true });
    };
    return () => {
        channel.onmessage = null;
        try {
            channel.close();
        } catch {
            // Already closed.
        }
    };
}
