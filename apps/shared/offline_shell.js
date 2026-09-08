/// What a till serves from its own copy, and what it must not.
///
/// The whole of this product is arranged so a shop can sell with the internet
/// down: the ledger is on the device, the catalogue is on the device, the
/// cashier signs in on the device, and what was rung waits in a log until the
/// shop can be reached. None of that is reachable if the tablet is switched off
/// and on during the outage, because the browser then has to fetch the page,
/// the script and the wasm from a server that is not answering. Walked, with
/// the app's own server stopped: the till showed a browser error page, and
/// every offline thing underneath it might as well not exist.
///
/// So the app keeps a copy of itself. The rules for using that copy are here,
/// as plain functions over a request, because they are the kind of thing that
/// is easy to get subtly wrong and impossible to notice: a till serving last
/// month's code against this month's server, or answering a sync request out of
/// a cache and telling a shop its sales were sent.
///
/// Nothing here talks to a browser. The service worker is the thin part that
/// does, and it decides nothing.

/// Where a request is answered from.
///
/// - `shell`: the app's own files, served from the copy first. They change only
///   when a new build is installed, and a shop mid-outage needs them more than
///   it needs them fresh.
/// - `shop`: anything addressed to the shop's server. Never from a copy, at any
///   price: a cached answer to "here are my sales" is a till told its sales
///   arrived when they did not, and a cached catalogue is a shelf priced wrong.
/// - `elsewhere`: not ours, so not our business.
export function answeredFrom(url, { origin, apiPrefixes = ['/v1/'] } = {}) {
  let where;
  try {
    where = new URL(url, origin ?? 'http://localhost');
  } catch {
    return 'elsewhere';
  }
  if (origin && where.origin !== new URL(origin).origin) return 'elsewhere';
  if (apiPrefixes.some((prefix) => where.pathname.startsWith(prefix))) return 'shop';
  return 'shell';
}

/// Only ordinary reads are served from a copy.
///
/// A POST is somebody doing something, and doing it twice out of a cache is at
/// best a duplicate. This is belt and braces beside `answeredFrom`, because the
/// two rules protect against different mistakes: one about where a request is
/// going and one about what kind of request it is.
export function mayBeServedFromACopy(request) {
  return (request?.method ?? 'GET') === 'GET';
}

/// The name of the copy this build of this app owns.
///
/// Keyed on the build, so installing a new one does not edit the old one's
/// files underneath a till that is still running them. The old copy is deleted
/// only when the new build takes over, which is the moment nothing is reading
/// it.
///
/// And keyed on the app, which is not decoration. The till and the back office
/// are served from one origin, and a browser's caches belong to the origin
/// rather than to the worker's scope: named on the build alone, each app's
/// worker deleted the other app's copy every time it took over a new build.
/// The back office is the one that would notice, because it is opened once a
/// week and by then the till has replaced its build several times over: it is
/// the likeliest of the two to be opened on the morning the line is down, and
/// it was the likeliest to find its copy gone. Found by watching two caches sit
/// on one origin during a walk.
export function copyNamed(build, base = '/') {
  return `openpos-shell-${appNamed(base)}-${build}`;
}

/// Which app a base path is, as a name a cache can carry.
function appNamed(base) {
  const trimmed = String(base ?? '/').replace(/^\/+|\/+$/g, '');
  return trimmed === '' ? 'till' : trimmed.replace(/[^a-z0-9]+/gi, '-').toLowerCase();
}

/// The copies to delete when a new build takes over.
///
/// This app's older builds, and nothing else. Another app's copy is not ours to
/// delete, and neither is a cache that is not ours at all.
export function copiesToForget(existing, build, base = '/') {
  const mine = `openpos-shell-${appNamed(base)}-`;
  const keep = copyNamed(build, base);
  return (existing ?? []).filter((name) => name.startsWith(mine) && name !== keep);
}

/// Whether a new build may take over now.
///
/// Never mid-sale. A service worker that swaps the running code the moment it
/// downloads it is a till whose screen reloads while a cashier is halfway
/// through a basket, and in the worst case it is a basket that was rung against
/// one version of the pricing rules and finished against another.
///
/// So a new build waits for the till to be doing nothing worth interrupting: no
/// basket on the screen, no drawer counting in progress, nothing unsent. The
/// ordinary answer is that it takes over the next time the shop opens the app,
/// which is what a service worker does by default and what this exists to
/// preserve rather than defeat.
export function mayTakeOverNow({ lines = 0, tendered = false, counting = false, unsent = 0 } = {}) {
  return lines === 0 && !tendered && !counting && unsent === 0;
}

/// Everything a build needs before it can be opened with the internet down.
///
/// The list comes from the build itself rather than being written by hand: a
/// hand-written list is a list missing the file the bundler renamed, and the
/// symptom is a till that works everywhere except the shop it was installed in.
///
/// The wasm is the one nobody remembers. It is fetched by the script rather
/// than named in the page, so a list built by reading the HTML would leave out
/// the entire core and the failure would look like a blank screen.
export function shellFiles(paths, { base = '/' } = {}) {
  const wanted = (paths ?? [])
    .filter((path) => !path.endsWith('.map'))
    .filter((path) => !path.endsWith('sw.js'))
    .map((path) => (path.startsWith('/') ? path : `${base}${path}`.replace(/\/{2,}/g, '/')));
  // The page itself, under the address somebody actually opens. A copy holding
  // /index.html and asked for / is a copy that answers nothing.
  const home = base.endsWith('/') ? base : `${base}/`;
  return [...new Set([home, ...wanted])].sort();
}

/// What to answer with when a request for the page misses the copy.
///
/// A single-page app is opened at addresses that are not files: the till at `/`
/// and the back office at `/admin/`, and any path inside either. Those all mean
/// the same page, so a miss on one of them is answered with the page rather
/// than with a browser error, which is the whole failure this file exists to
/// prevent.
export function fallbackFor(url, { base = '/', origin } = {}) {
  if (answeredFrom(url, { origin }) !== 'shell') return null;
  let path;
  try {
    path = new URL(url, origin ?? 'http://localhost').pathname;
  } catch {
    return null;
  }
  // A file that is missing is missing. Answering a request for a script with
  // the page gives a syntax error somewhere else entirely, which is a worse
  // thing to debug than a 404.
  if (/\.[a-z0-9]+$/i.test(path)) return null;
  return base.endsWith('/') ? base : `${base}/`;
}
