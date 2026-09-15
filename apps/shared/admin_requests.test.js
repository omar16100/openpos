/// Every request the back office can build is asked for by a screen.
///
/// The second link in a chain the server holds the first of. A route nothing
/// posts to is caught in `server/tests/every_route_is_reachable.rs`; a request
/// nothing asks for is caught here, and that is the layer where all four of
/// this month's dead-code defects actually lived. An item could not be deleted
/// because no screen asked, and the handler had tests, and the tests passed.
///
/// The list is written out by the Rust test from the enum itself, because the
/// JavaScript cannot read Rust and a list somebody copied by hand goes stale
/// the first time a request is added.
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { everyScreen } from './screens.js';

const REQUESTS = JSON.parse(
  readFileSync(new URL('./admin_requests.json', import.meta.url), 'utf8'),
);

/// The shared code that builds a request on a screen's behalf. The screens
/// themselves come from everyScreen(), which reads all of both rather than the
/// two files that happened to hold them when this was written.
const CALLERS = [
  './counting.js',
  './buying.js',
  './records.js',
  './people.js',
  './repricing.js',
  './till.js',
];

function asked() {
  const found = new Set();
  const sources = [
    ...everyScreen().map((screen) => screen.source),
    ...CALLERS.map((caller) => readFileSync(new URL(caller, import.meta.url), 'utf8')),
  ];
  for (const source of sources) {
    for (const [, what] of source.matchAll(/what:\s*'([a-z_]+)'/g)) found.add(what);
  }
  return found;
}

test('every request the back office can build is asked for by a screen', () => {
  const wanted = asked();
  for (const request of REQUESTS) {
    assert.ok(
      wanted.has(request),
      `nothing asks for '${request}'. The server serves it, the bindings build it, and it has ` +
        `tests: it is finished, green, and does nothing for a shop. Either a screen is missing, ` +
        `which is the defect this test exists for, or it should come out.`,
    );
  }
});

test('every request a screen asks for is one the back office can build', () => {
  // The other direction. A screen asking for something the bindings do not
  // build gets a refusal that reads as the shop being down, and a typo in one
  // of these is invisible until somebody presses that button.
  const known = new Set(REQUESTS);
  for (const what of asked()) {
    assert.ok(
      known.has(what),
      `a screen asks for '${what}' and the bindings build no such request: whoever presses that ` +
        `button is told the shop could not be reached`,
    );
  }
});
