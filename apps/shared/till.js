// Talking to the till worker.
//
// Two things here are about a page going away rather than about a till: the
// files are let go on the way out, and an open waits through a handover that
// has not finished. See `opening_again.js` for why both exist.
//
// One promise per command, matched by id, because a barcode scanner can fire
// faster than a round trip and replies that arrive out of order would otherwise
// render the wrong basket.

import { openingAgain } from './opening_again.js';

const pending = new Map();
let nextId = 1;
let worker = null;
let makeWorker = null;
/// What to call when the worker says something nobody asked for.
let onEvent = null;

/// How this app makes its worker.
///
/// Passed in rather than built here, because the bundler resolves a worker URL
/// against the file that writes it, and each app's worker entry is what hands
/// the core its own copy of the wasm under its own base path.
export function useWorker(factory) {
  makeWorker = factory;
}

function ensureWorker() {
  if (worker) return worker;
  if (!makeWorker) throw new Error('no worker was set up for this app');
  worker = makeWorker();
  worker.onmessage = (event) => {
    const { id, ok, view, info, error, error_code, error_parts } = event.data;
    // A message nobody asked for: the sync loop, which lives in the worker so a
    // till in a background tab keeps sending. Everything else here is matched to
    // a request by id, and an unmatched reply used to be dropped on the floor.
    if (event.data.event) {
      // The worker asking whether anybody is still at this page. Answered here
      // rather than by a screen, because a screen that forgot to answer would
      // have its till taken out from under it, and because the answer is the
      // same for every app: this page is running, or it is not answering at
      // all. See `still_someone_there.js` for what silence means.
      if (event.data.event === 'still_there') {
        worker.postMessage({ id: nextId++, kind: 'still_here' });
        return;
      }
      onEvent?.(event.data);
      return;
    }
    const waiting = pending.get(id);
    if (!waiting) return;
    pending.delete(id);
    if (ok) {
      waiting.resolve({ view, info });
      return;
    }
    // The view rides along on the failure, because a request that failed still
    // changed what the till knows and a caller that catches this should be able
    // to show it.
    const refusal = new Error(error);
    refusal.view = view;
    // The name the shop's server gave the refusal, and its figures, so the
    // screen can word it in the shop's language. The message is the English
    // fallback and stays that.
    if (error_code) {
      refusal.code = error_code;
      refusal.parts = error_parts ?? {};
    }
    waiting.reject(refusal);
  };
  return worker;
}

function send(kind, payload) {
  const id = nextId++;
  ensureWorker().postMessage({ id, kind, payload: plain(payload) });
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

/// A copy the structured clone algorithm will accept.
///
/// Anything read back out of the view is a reactive proxy, and a proxy cannot
/// be posted to a worker: it throws "could not be cloned" and the command never
/// runs. That is exactly what happens when a screen hands back something the
/// core gave it, which is the whole shape of a refusal a supervisor allows: the
/// core names the action, the screen sends it back as it stands, and the send
/// failed with a message about postMessage rather than doing anything.
///
/// Here rather than at each screen, because a screen that forgets is a screen
/// that works until somebody tries the one path that reads from the view.
/// Strings pass through untouched, which is what the large payloads are.
export function plain(payload) {
  if (payload === null || typeof payload !== 'object') return payload;
  return JSON.parse(JSON.stringify(payload));
}

/// Open the till. `durable: false` keeps everything in memory.
export function open(tenant, terminal, durable = true) {
  // Waited through rather than reported. A browser can go on holding a shop's
  // ledger for a window that has already gone, and the gap is short: see
  // `opening_again.js`. Everything other than "somebody else has these" is
  // thrown at once, so a device with no room still says so immediately.
  return openingAgain(() => send('open', { tenant, terminal, durable }));
}

/// Tell the till which build it is running, so the shop can see it.
///
/// The build is a hash of everything in the copy the app keeps of itself, and
/// the service worker is the only thing that knows it: the page was built
/// before that hash existed. A device with no service worker, which is any
/// browser refusing one and every development reload before the first install,
/// says nothing rather than guessing, and the shop shows nothing for it.
///
/// Not waited for by anything. A till whose build is unknown sells exactly as
/// it did; what is lost is a line on a support screen.
export async function sayWhichBuild({ timeoutMs = 2_000 } = {}) {
    const worker = globalThis.navigator?.serviceWorker;
    if (!worker?.controller) return null;
    const build = await new Promise((settle) => {
        const done = (event) => {
            if (event.data?.openpos !== 'build') return;
            worker.removeEventListener('message', done);
            settle(event.data.build ?? null);
        };
        worker.addEventListener('message', done);
        worker.controller.postMessage('which-build');
        setTimeout(() => {
            worker.removeEventListener('message', done);
            settle(null);
        }, timeoutMs);
    });
    if (!build) return null;
    await send('built_as', { build });
    return build;
}

/// Let the files go, because this page is going away.
///
/// Called from `pagehide`, which is the event that fires whether the tab is
/// closed, reloaded, or put to sleep in the back-forward cache. `unload` is not
/// used: it does not fire reliably on mobile, which is the whole of the market.
///
/// The reply is not waited for and there is nothing to do about a failure: the
/// page is leaving either way, and what this buys is the next page opening at
/// once instead of being told to switch the device off and on.
/// Every `pagehide`, including the one that means the browser is keeping this
/// page. That exception used to be here, on the reasoning that a page the
/// browser intends to bring back exactly as it was runs nothing on the way in,
/// so letting go would strand it. The reasoning was wrong on its second half,
/// and measurably: a frozen page fires `pageshow` with `persisted` true when it
/// is restored, which is the chance to open again, and `openAgainOnTheWayIn`
/// below takes it.
///
/// What the exception cost is worth stating, because it is the failure a shop
/// actually meets. A page the browser has frozen is alive: it holds the lock on
/// this terminal's store and the handles on its files, and it cannot answer
/// anybody, because frozen is frozen. The next window waits out the whole of
/// its patience and is then told the till is open in another window on this
/// device. There is no such window. There is nothing to close, nothing to
/// switch to, and no way for the person standing at the counter to know that
/// the window they are being sent to find is a page the browser kept for the
/// back button. The till does not open until the browser lets that page go,
/// which can be minutes, and a shop cannot sell across it.
export function letGoOnTheWayOut() {
  if (typeof window === 'undefined') return;
  window.addEventListener('pagehide', () => {
    if (!worker) return;
    worker.postMessage({ id: nextId++, kind: 'let_go' });
  });
}

/// Open the ledger again, because the browser brought this page back.
///
/// The other half of letting go on the way out. A restored page has its screen,
/// its worker and its belief that the till is open, and no files: they were let
/// go when it was frozen, so that another window could sell. Nothing is pressed
/// between the two, because `pageshow` runs before the person can touch
/// anything, and the handler is the same one behind "try again" on the screen
/// that says the till is open elsewhere. So a page coming back either opens or
/// says what a page that cannot open always says.
export function openAgainOnTheWayIn(comeBack) {
  if (typeof window === 'undefined') return;
  window.addEventListener('pageshow', (event) => {
    if (!event.persisted) return;
    comeBack();
  });
}

/// Run one command and get the view back.
export function run(command) {
  return send('run', command);
}

/// Point the till at a server. No credential passes through here: the till holds
/// its own, beside its ledger, and hands it over with each request it builds.
export function connect(server) {
  return send('connect', { server });
}

/// Trade an enrolment code for a credential. Answers with the identity the code
/// gave this device, which is what a till must then be opened as.
export function enrol(code) {
  return send('enrol', { code });
}

/// Store the credential in a till that has just been opened with that identity.
export function adoptToken(token, at_ms = Date.now()) {
  return send('adopt', { token, at_ms });
}

/// What a pasted bundle hashes to, by the same code that marked it on the
/// device it came from. Empty when the paste is not a bundle.
export function bundleMark(bundle) {
  return send('mark', { bundle });
}

/// What each role a shop can pick means, asked of the core rather than held.
///
/// The back office held its own copy and the two disagreed, in a way nothing
/// would have complained about until somebody relied on the wrong one.
export function rolesOffered() {
  return send('roles', {});
}

/// Ask the core to build a back-office request, post it, and hand back what
/// came out. The same three moves as everything else, so the back office knows
/// no more about the protocol than the till does.
export function admin(request, nowMs) {
  return send('admin', { request, now_ms: nowMs });
}

/// What the sync loop is doing, in words a shopkeeper can act on.
///
/// Shared by the till and the back office, because a till that has stopped
/// reaching the shop must say so wherever it is looked at, and two copies of
/// this would be two chances to describe it as "idle".
export function describeSync(outcome) {
  // A key and its figures rather than a sentence: the shop screens say this in
  // the language the shop reads, and a sentence built here could only ever be
  // English. The kinds a round can report are the protocol's own words (pull,
  // customers, report_drawer) and mean nothing at a counter, so they collapse
  // into the two states somebody there cares about: sending what was rung, and
  // catching up with the shop.
  if (outcome?.did) {
    return { key: outcome.did === 'push' ? 'sync.sending' : 'sync.reading', fill: {} };
  }
  const failures = outcome?.info?.after_failures ?? outcome?.after_failures ?? 0;
  if (failures === 0) return { key: 'sync.idle', fill: {} };
  const seconds = Math.max(1, Math.round((outcome?.info?.waited ?? outcome?.waited ?? 0) / 1000));
  // What is waiting to be sent is already on the screen, from the view. Saying
  // it again here meant two numbers taken at two moments, and they disagreed.
  return { key: 'sync.not_reaching', fill: { seconds } };
}

/// Why a round failed, in a key a screen can say in the shop's language.
///
/// A round that fails carries whatever the browser or the shop said. Most of
/// those are refusals with a name and figures, and the screen words them from
/// the dictionary. What is left is the commonest one of all: the shop cannot be
/// reached, which arrives as `TypeError: Failed to fetch`, two English words a
/// browser chose. That is what a shopkeeper reads on the first failure of an
/// outage, in the middle of a Bangla sentence, on the one screen state this
/// whole product exists for.
///
/// `null` for anything that has a name of its own, because the dictionary says
/// those better than this could.
export function whyTheRoundFailed(error, code) {
  if (code) return null;
  const said = String(error ?? '');
  // Chrome says "Failed to fetch", Firefox "NetworkError when attempting to
  // fetch resource", Safari "Load failed". Matched on all three rather than on
  // one, because the browser a shop uses is not this project's decision.
  if (/failed to fetch|networkerror|load failed|network request failed/i.test(said)) {
    return 'sync.cannot_reach_the_shop';
  }
  return null;
}

/// One round of the sync loop.
export function sync(nowMs) {
  return send('sync', { now_ms: nowMs });
}

/// Let the worker sync on its own, and say what it did each round.
///
/// The loop was a timer on the screen's thread, which a browser throttles to
/// about once a minute when the tab is not in front and can stop altogether. A
/// till that has quietly stopped sending is the failure this design exists to
/// prevent, so the loop belongs where the till is.
export function keepSyncing(watch, everyMs = 2000) {
  onEvent = (message) => {
    if (message.event !== 'synced') return;
    watch(message);
  };
  return send('sync_loop', { every_ms: everyMs });
}
