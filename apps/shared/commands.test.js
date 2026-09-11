import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { needsAnOpenTill } from './commands.js';

test('the sync loop can be armed before this device knows who it is', () => {
  // The one that bit. A till enrolling for the first time asks for the loop as
  // it boots: refusing it left the loop unstarted, and the device sat saying
  // nobody had been added to the shop until somebody reloaded the page.
  assert.equal(needsAnOpenTill('sync_loop'), false);
});

test('what has to happen before a till exists does not wait for one', () => {
  // `roles` is here because the back office asks what each role means as it
  // boots, and a device that has not enrolled yet still has to be able to add
  // the first person to the shop.
  for (const kind of ['connect', 'open', 'enrol', 'mark', 'roles', 'sync_loop']) {
    assert.equal(needsAnOpenTill(kind), false, kind);
  }
});

test('anything that touches the ledger waits for it', () => {
  for (const kind of ['scan', 'checkout', 'sync', 'admin', 'adopt', 'remove_line']) {
    assert.equal(needsAnOpenTill(kind), true, kind);
  }
});

test('a command this build has never heard of waits, rather than being let past', () => {
  // The safe direction. A newer screen asking for something older code does not
  // know should meet the till's own refusal, not a null.
  assert.equal(needsAnOpenTill('something_from_next_year'), true);
});
