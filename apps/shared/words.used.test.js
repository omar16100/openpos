/// Every word a screen asks for exists, and every word this file holds is asked
/// for.
///
/// A missing key falls back to English on purpose, so a screen older than the
/// core it talks to still says something. That fallback also means a typo in a
/// key is invisible: the screen shows English, in a shop that chose Bangla, and
/// nothing anywhere complains. So the source is scanned.
///
/// The other direction matters too. A dictionary keeps every phrase anybody
/// ever wrote, including the ones for screens that changed, and each of those is
/// a line somebody has to translate again when a language is added.
///
/// Source scanning rather than anything cleverer, for the same reason
/// `frozen_shapes.rs` scans `wire.rs`: the property is about what is written in
/// the file.
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { WORDS } from './words.js';

/// Where a key can be asked for: the two screens, and the shared code that
/// hands a screen a key to say (the sync line does exactly that).
const SCREENS = ['../till-web/src/App.svelte', '../admin/src/App.svelte', './till.js'];

/// Keys asked for by name, which is every ordinary call. Two shapes are asked
/// for by a name built at run time and are listed here instead: a tender kind
/// (`till.cash` and friends) and what is wrong with a row of a spreadsheet
/// (`file.no-name` and friends), both of which come from data.
const BUILT_AT_RUN_TIME = /^(till\.(cash|card|credit)|file\.|allowed\.)/;

function asked() {
  const found = new Set();
  for (const screen of SCREENS) {
    const source = readFileSync(new URL(screen, import.meta.url), 'utf8');
    for (const [, key] of source.matchAll(/\bt\(\s*'([a-z0-9_.-]+)'/g)) found.add(key);
    // A key handed to a screen to say, rather than said here: the sync line.
    for (const [, key] of source.matchAll(/key:\s*'([a-z0-9_.-]+)'/g)) found.add(key);
    // Both sides of a choice between two keys, which is how the sync line picks
    // between sending and catching up.
    for (const [, key] of source.matchAll(/'(sync\.[a-z_]+)'/g)) found.add(key);
    for (const [, key] of source.matchAll(/\bsay\(\s*[a-zA-Z']+\s*,\s*'([a-z0-9_.-]+)'/g)) {
      found.add(key);
    }
  }
  return found;
}

test('every word a screen asks for is in the dictionary', () => {
  for (const key of asked()) {
    assert.ok(
      WORDS[key],
      `${key} is asked for by a screen and this file does not hold it. The screen would show ` +
        `the key itself, or English in a shop that chose Bangla, and nothing would complain.`,
    );
  }
});

test('every word in the dictionary is asked for by something', () => {
  // The refusal codes are covered by their own test against refusals.json,
  // because they are asked for by code rather than by name.
  const refusals = new Set(
    JSON.parse(readFileSync(new URL('./refusals.json', import.meta.url), 'utf8')),
  );
  const wanted = asked();
  for (const key of Object.keys(WORDS)) {
    if (refusals.has(key) || BUILT_AT_RUN_TIME.test(key)) continue;
    assert.ok(
      wanted.has(key),
      `${key} is in the dictionary and no screen asks for it. Take it out, or somebody has to ` +
        `translate it again for every language this shop ever speaks.`,
    );
  }
});
