/// Register the app's own copy of itself, and apply a new build safely.
///
/// The browser half of `offline_shell.js`, kept apart from it so the rules stay
/// testable outside a browser. Nothing is decided here: when a new build may
/// take over is `mayTakeOverNow`, and it has tests.
import { mayTakeOverNow } from './offline_shell.js';

/// How often to ask whether the till is quiet enough to swap the code under it.
const LOOK_EVERY_MS = 20_000;

/// Keep a copy of this app, and watch for a newer one.
///
/// `whatTheTillIsDoing` is asked before a new build is allowed to take over. It
/// answers with a basket's line count, whether money is already on the ticket,
/// whether somebody is counting the drawer, and how many sales are unsent.
///
/// Returns nothing worth holding: it runs for the life of the page.
export function keepACopy(whatTheTillIsDoing, say = () => {}) {
  if (!('serviceWorker' in navigator)) {
    // Not a failure worth showing anybody: it means this browser cannot keep a
    // copy, so the app needs the shop's server to open. Said in the console
    // because whoever is standing there cannot do anything about it.
    console.log('[openpos] this browser keeps no copy: the app needs the server to open');
    return;
  }

  navigator.serviceWorker
    .register(new URL('sw.js', document.baseURI), { scope: './' })
    .then((registration) => {
      console.log('[openpos] keeping a copy of this build');
      watch(registration, whatTheTillIsDoing, say);
    })
    .catch((trouble) => {
      // Worth a line and no more. A till that could not keep a copy still
      // sells; it just cannot be opened during an outage, which is the thing
      // the shop finds out at the worst moment, so it is said out loud here.
      console.log(`[openpos] could not keep a copy: ${trouble?.message ?? trouble}`);
    });

  // A reload the browser did on its own, because a new build took over. Doing
  // it here rather than leaving the page running against a copy that is no
  // longer the one on disk.
  let swapped = false;
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (swapped) return;
    swapped = true;
    console.log('[openpos] a new build took over: reloading');
    window.location.reload();
  });
}

/// Watch for a build that is ready and waiting, and let it in when it is safe.
function watch(registration, whatTheTillIsDoing, say) {
  const offer = () => {
    const waiting = registration.waiting;
    if (!waiting) return;
    say(true);
    // Never mid-basket. A tab that is never closed would otherwise wait for
    // ever, and one that swaps the moment it can would reload under a cashier.
    if (mayTakeOverNow(whatTheTillIsDoing())) {
      waiting.postMessage('take-over');
    }
  };

  registration.addEventListener('updatefound', () => {
    const arriving = registration.installing;
    arriving?.addEventListener('statechange', () => {
      if (arriving.state === 'installed') offer();
    });
  });

  offer();
  setInterval(() => {
    // Asking is cheap and finding out late is not: a till left on for a month
    // would otherwise run the build it was installed with.
    registration.update().catch(() => {});
    offer();
  }, LOOK_EVERY_MS);
}
