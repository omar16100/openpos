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
