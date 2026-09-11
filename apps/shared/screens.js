/// Where the screens are, for the tests that read them.
///
/// Several guards here work by reading a screen's own source: no screen writes
/// down what a role may do, no screen builds a date in Greenwich, no screen
/// asks for a paper in anything but English, every request a screen sends is
/// one the shop answers. All of them named `apps/admin/src/App.svelte` and
/// `apps/till-web/src/App.svelte` directly.
///
/// A screen that outgrows one file is a screen those guards stop guarding, and
/// they stop quietly: the named file still exists, still passes, and the half
/// that moved out of it is read by nobody. That is the worst failure a guard
/// has, because the suite goes on being green.
///
/// So a screen is a directory, and these read all of it.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/// The two screens, as directories rather than as files.
const WHERE = ['../admin/src', '../till-web/src'];

/// What a screen is written in. Not `.js`, because a screen's own logic lives
/// in its component and the shared modules beside this file are tested
/// directly rather than scanned.
const IS_A_SCREEN = /\.svelte$/;

function walk(directory, found) {
  for (const entry of readdirSync(directory)) {
    const path = `${directory}/${entry}`;
    if (statSync(path).isDirectory()) {
      walk(path, found);
      continue;
    }
    if (IS_A_SCREEN.test(entry)) found.push(path);
  }
  return found;
}

/// Every file that makes up a screen, as `{ path, source }`.
///
/// `path` is what a failing assertion prints, so it is the repository path a
/// person can open rather than an absolute one from this machine.
export function everyScreen() {
  const found = [];
  for (const where of WHERE) {
    const root = fileURLToPath(new URL(where, import.meta.url));
    for (const file of walk(root, [])) {
      found.push({
        path: file.slice(file.indexOf('/apps/') + 1),
        source: readFileSync(file, 'utf8'),
      });
    }
  }
  return found;
}

/// The files of one screen: 'admin' or 'till-web'.
export function screenOf(which) {
  return everyScreen().filter((file) => file.path.includes(`/${which}/`));
}
