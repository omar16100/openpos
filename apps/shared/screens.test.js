import { test } from 'node:test';
import assert from 'node:assert/strict';

import { everyScreen, screenOf } from './screens.js';

/// The guard on the guards.
///
/// Everything else here reads the screens through this, so a change that made
/// it find nothing would turn every source-reading test green and empty at
/// once. That is the failure worth a test of its own: a suite that passes
/// because it is looking at nothing.
test('both screens are found, and they are not empty', () => {
  const found = everyScreen();
  assert.ok(found.length >= 2, `only ${found.length} screen file(s) found; both screens exist`);

  for (const which of ['admin', 'till-web']) {
    const files = screenOf(which);
    assert.ok(files.length > 0, `no files found for the ${which} screen`);
    // A file that reads as empty is one this has stopped finding properly.
    for (const file of files) {
      assert.ok(file.source.length > 100, `${file.path} came back all but empty`);
    }
  }
});

test('a screen split into files is still all of it', () => {
  // Both screens have a component at their root today. What this asserts is
  // that whatever is beside it is picked up too: the paths come back as
  // repository paths under apps/, so a nested panel reads the same way.
  for (const file of everyScreen()) {
    assert.match(file.path, /^apps\/(admin|till-web)\/src\/.*\.svelte$/, file.path);
  }
  const paths = everyScreen().map((file) => file.path);
  assert.equal(new Set(paths).size, paths.length, 'a file was read twice');
});

/// A paper handed to a customer totals the whole account, or is not printed.
///
/// The khata page adds up the lines it is handed, and the screen holds one page
/// of fifty. A customer with more entries than that was handed a slip saying
/// they owed the sum of the newest fifty, which for anybody who has been paying
/// along the way is far too little and can read as being in credit. It is a
/// document the shop hands over and the customer holds it to.
///
/// So the print path has to consult whether the account is all here before it
/// asks for paper. Scanned rather than run, because the alternative is a
/// browser: what it holds is that the two facts are wired together at all, and
/// the sentence that refuses is in the dictionary under its own key.
test('printing somebody the account they take away consults whether it is all there', () => {
  const panel = screenOf('admin').find((file) => file.path.endsWith('accounts.svelte'));
  assert.ok(panel, 'the account panel is where it was');

  const at = panel.source.indexOf('async function printAccount');
  assert.ok(at > 0, 'and it still prints an account');
  const upToTheAsk = panel.source.slice(at, panel.source.indexOf('statement_paper', at));
  assert.ok(upToTheAsk.length > 0, 'which still asks the core for paper');
  assert.match(
    upToTheAsk,
    /accountComplete/,
    'a page printed from part of an account carries a total the shop cannot stand behind'
  );
});
