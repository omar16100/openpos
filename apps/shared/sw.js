/// The app's own copy of itself, so a shop can open the till with the internet
/// down.
///
/// Deliberately thin, and it decides nothing. Every rule it follows is in
/// `offline_shell.js`, which is plain functions with tests: this file is the
/// part that cannot be tested outside a browser, so there is as little of it as
/// possible. The same arrangement as `till.worker.js`, for the same reason.
///
/// The two lines below are filled in by `scripts/stage-apps.sh` from the build
/// it just made. A list written by hand is a list missing the file the bundler
/// renamed, and the shop finds out when the tablet is switched on during an
/// outage and shows a browser error.

// __OPENPOS_BUILD__
const BUILD = 'unbuilt';
const BASE = '/';
const FILES = [];

// __OPENPOS_RULES__
// The rules from offline_shell.js are pasted in here by the build, with their
// `export` keywords taken off. Pasted rather than imported because a service
// worker importing a sibling module has to be a module worker served from a
// path that resolves, and the alternative to pasting is a second bundler entry
// point: one source of truth either way, and this one is a `sed`.

const COPY = copyNamed(BUILD, BASE);

/// Take a copy of this build. Every file, or none: a half-copied app is worse
/// than no copy, because it boots and then fails on whatever is missing.
self.addEventListener('install', (event) => {
  event.waitUntil(
    (async () => {
      const copy = await caches.open(COPY);
      await copy.addAll(FILES);
      console.log(`[openpos] build ${BUILD} copied, ${FILES.length} files`);
    })(),
  );
});

/// Take over, and forget the copies of builds nobody is running.
///
/// `skipWaiting` is deliberately not called. A worker that takes over the
/// moment it installs swaps the running code under a cashier mid-basket, and in
/// the worst case a sale is rung under one version of the pricing rules and
/// finished under another. The default is that a new build waits for every tab
/// to close, which for a till is the next time the shop opens it: that is the
/// behaviour wanted, and this exists to preserve it rather than defeat it.
self.addEventListener('activate', (event) => {
  event.waitUntil(
    (async () => {
      for (const name of copiesToForget(await caches.keys(), BUILD, BASE)) {
        await caches.delete(name);
        console.log(`[openpos] forgot ${name}`);
      }
      await self.clients.claim();
    })(),
  );
});

/// Take over now, because the app has said the till is doing nothing.
///
/// The counterpart to not calling `skipWaiting` on install. A till's tab is
/// never closed, so a new build would otherwise wait for a shop to close for
/// the night and reopen, which on a tablet nobody switches off is for ever. The
/// app watches for a moment with no basket, no money on the ticket, nobody
/// counting and nothing unsent, and says so; the decision is `mayTakeOverNow`
/// in offline_shell.js, where it can be tested.
self.addEventListener('message', (event) => {
  if (event.data === 'take-over') {
    console.log('[openpos] the till says it is idle: taking over');
    self.skipWaiting();
  }
});

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (!mayBeServedFromACopy(request)) return;
  const where = answeredFrom(request.url, { origin: self.location.origin });
  // The shop's own server is never answered from a copy, at any price: a cached
  // answer to a push is a till told its sales arrived when they did not.
  if (where !== 'shell') return;

  event.respondWith(
    (async () => {
      const copy = await caches.open(COPY);
      const held = await copy.match(request);
      if (held) return held;

      // Not in the copy: a file added since, or a path inside the app that is
      // not a file at all.
      try {
        const fresh = await fetch(request);
        // Kept, so the second outage does not lose what the first one had.
        if (fresh.ok) await copy.put(request, fresh.clone());
        return fresh;
      } catch (unreachable) {
        const instead = fallbackFor(request.url, {
          base: BASE,
          origin: self.location.origin,
        });
        if (instead) {
          const page = await copy.match(instead);
          if (page) return page;
        }
        console.log(`[openpos] ${request.url} is not in this build's copy and the shop is not reachable`);
        throw unreachable;
      }
    })(),
  );
});
