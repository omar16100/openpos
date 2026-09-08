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

/// Keys asked for by a name built at run time rather than written in a screen:
/// a tender kind, what is wrong with a row of a spreadsheet, what a till wrote
/// in its trail, and the labels the core prints on paper. The last of those has
/// its own test against `paper_words.json`, which the core writes out.
const BUILT_AT_RUN_TIME = /^(till\.(cash|card|credit)|file\.|allowed\.|held\.|unit\.|paper:)/;

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

/// The message slots a screen puts a sentence into for somebody to read.
const SAID_TO_SOMEBODY = /\b(fault|done|note)\s*=\s*(['"`])/g;

/// The literal that starts at `at`, quote and all, honouring escapes.
function literalAt(source, at) {
  const quote = source[at];
  let end = at + 1;
  while (end < source.length) {
    if (source[end] === '\\') end += 2;
    else if (source[end] === quote) return source.slice(at + 1, end);
    else end += 1;
  }
  return '';
}

test('no screen says a sentence of its own', () => {
  // The two tests above hold the dictionary honest and hold the keys honest.
  // Neither notices a screen that skips the dictionary altogether and assigns
  // English straight to the line somebody reads, which is how five sentences
  // survived the translation: "somebody who can sign in is already called
  // that", and the count at the end of an import, among them. A shop that
  // chose Bangla read them in English and nothing anywhere complained.
  //
  // Two words in a row is the test. A slot set to a name, a mark or a figure
  // is not a sentence, and neither is a template that is nothing but the
  // pieces it interpolates.
  for (const screen of SCREENS) {
    const source = readFileSync(new URL(screen, import.meta.url), 'utf8');
    for (const found of source.matchAll(SAID_TO_SOMEBODY)) {
      const held = literalAt(source, found.index + found[0].length - 1);
      const prose = held.replace(/\$\{[^}]*\}/g, ' ');
      assert.ok(
        !/[A-Za-z]{2,}\s+[A-Za-z]{2,}/.test(prose),
        `${screen} says "${held.slice(0, 60)}" itself instead of asking words.js for it. A shop ` +
          `that reads Bangla would read that line in English, and no other test here would ` +
          `notice.`,
      );
    }
  }
});

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
