import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import {
  answeredFrom,
  copiesToForget,
  copyNamed,
  fallbackFor,
  mayBeServedFromACopy,
  mayTakeOverNow,
  shellFiles,
} from './offline_shell.js';

const AT = 'http://shop.example';

test('the shop’s own server is never answered from a copy', () => {
  // The one that would be a disaster rather than an inconvenience. A cached
  // answer to a push is a till told its sales reached the shop when they did
  // not, and the log it would have kept them in is emptied on that answer.
  assert.equal(answeredFrom(`${AT}/v1/sync/push`, { origin: AT }), 'shop');
  assert.equal(answeredFrom(`${AT}/v1/sync/pull`, { origin: AT }), 'shop');
  assert.equal(answeredFrom(`${AT}/v1/back-office/day`, { origin: AT }), 'shop');
});

test('the app’s own files come from the copy', () => {
  assert.equal(answeredFrom(`${AT}/`, { origin: AT }), 'shell');
  assert.equal(answeredFrom(`${AT}/assets/main-a1b2.js`, { origin: AT }), 'shell');
  assert.equal(answeredFrom(`${AT}/pkg/openpos_bindings_bg.wasm`, { origin: AT }), 'shell');
  assert.equal(answeredFrom(`${AT}/admin/`, { origin: AT }), 'shell');
});

test('somebody else’s server is somebody else’s business', () => {
  assert.equal(answeredFrom('https://elsewhere.example/thing.js', { origin: AT }), 'elsewhere');
  assert.equal(answeredFrom('http://', { origin: AT }), 'elsewhere', 'nothing that can be parsed');

  // A worker is handed absolute addresses, so this next one cannot arrive in
  // practice. It is asserted as it behaves rather than as it reads: a bare
  // string is a path on our own origin, which is the app's own file and the
  // right answer. Written down because the alternative is somebody reading
  // `elsewhere` into it later and building on a guess.
  assert.equal(answeredFrom('some/relative/path', { origin: AT }), 'shell');
});

test('only an ordinary read may be answered from a copy', () => {
  // Beside the rule above rather than instead of it: the two protect against
  // different mistakes, one about where a request is going and one about what
  // it is. A POST replayed out of a cache is at best a duplicate.
  assert.equal(mayBeServedFromACopy({ method: 'GET' }), true);
  assert.equal(mayBeServedFromACopy({}), true, 'a request with no method is a GET');
  assert.equal(mayBeServedFromACopy({ method: 'POST' }), false);
  assert.equal(mayBeServedFromACopy({ method: 'DELETE' }), false);
});

test('a build keeps its own copy and forgets every other', () => {
  assert.equal(copyNamed('a1b2c3'), 'openpos-shell-a1b2c3');
  assert.deepEqual(
    copiesToForget(
      ['openpos-shell-old', 'openpos-shell-a1b2c3', 'something-else-entirely'],
      'a1b2c3',
    ),
    ['openpos-shell-old'],
    'the running build is kept, and a cache that is not ours is left alone',
  );
  assert.deepEqual(copiesToForget([], 'a1b2c3'), []);
});

test('a new build does not take over in the middle of a sale', () => {
  // A service worker that swaps the running code the moment it has it is a till
  // whose screen reloads while a cashier is halfway through a basket. Worse, it
  // is a basket rung under one version of the pricing rules and finished under
  // another.
  assert.equal(mayTakeOverNow({}), true, 'a till doing nothing');
  assert.equal(mayTakeOverNow({ lines: 3 }), false, 'a basket on the screen');
  assert.equal(mayTakeOverNow({ tendered: true }), false, 'money already on the ticket');
  assert.equal(mayTakeOverNow({ counting: true }), false, 'somebody counting the drawer');
  assert.equal(mayTakeOverNow({ unsent: 2 }), false, 'sales this build has not sent yet');
});

test('the list of files comes from the build and includes the address people open', () => {
  const files = shellFiles(['assets/main-a1b2.js', 'assets/main-a1b2.css', 'pkg/core_bg.wasm']);
  assert.deepEqual(files, [
    '/',
    '/assets/main-a1b2.css',
    '/assets/main-a1b2.js',
    '/pkg/core_bg.wasm',
  ]);

  // The back office lives under a path, and a copy that holds /admin/index.html
  // and is asked for /admin/ answers nothing.
  assert.ok(shellFiles(['assets/x.js'], { base: '/admin/' }).includes('/admin/'));
  assert.ok(shellFiles(['assets/x.js'], { base: '/admin/' }).includes('/admin/assets/x.js'));
});

test('source maps and the worker itself are not part of the copy', () => {
  // A map is for whoever is debugging and is often larger than the code. The
  // worker must never be served from the copy it manages, or a build can never
  // be replaced.
  assert.deepEqual(shellFiles(['a.js', 'a.js.map', 'sw.js']), ['/', '/a.js']);
});

test('a page opened at a path the copy does not hold still opens', () => {
  // Both apps are single-page: every address inside one means the same page. A
  // miss on one of them has to answer with the page, or reloading on any path
  // during an outage is the browser error this whole file exists to prevent.
  assert.equal(fallbackFor(`${AT}/anything/at/all`, { origin: AT }), '/');
  assert.equal(fallbackFor(`${AT}/admin/repairs`, { origin: AT, base: '/admin/' }), '/admin/');

  // But a missing file is missing. Answering a request for a script with the
  // page gives a syntax error somewhere else entirely, which is worse to debug
  // than a 404.
  assert.equal(fallbackFor(`${AT}/assets/gone-a1b2.js`, { origin: AT }), null);
  assert.equal(fallbackFor(`${AT}/pkg/core_bg.wasm`, { origin: AT }), null);

  // And the shop's server is never answered with a page.
  assert.equal(fallbackFor(`${AT}/v1/sync/push`, { origin: AT }), null);
});
