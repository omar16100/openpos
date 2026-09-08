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

/// The shared modules, which run on both screens and know no language at all.
const SHARED = ['./catalogue_file.js', './till.js', './counting.js', './buying.js'];

/// A sentence leaving a shared module: returned, or handed back as a fault.
const HANDED_BACK = /(\breturn\s+|\bfault:\s*)(['"`])/g;

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

/// What a screen writes into an attribute a person reads or a screen reader
/// speaks.
const IN_THE_MARKUP = /\b(placeholder|aria-label|title|alt)="([^"]*)"/g;

test('no screen writes English into an attribute', () => {
  // Sixteen of these survived the translation: every tooltip along the top of
  // the till, "how many" next to a quantity box, "% off", "Why: broken,
  // spoiled, taken, given away". A tooltip is read by whoever is unsure, and an
  // aria-label is the only thing a screen reader has, so these are among the
  // worst places for a language nobody in the shop reads.
  //
  // Invisible to the scan above, which looks at what a screen assigns to a
  // message slot. These are markup.
  for (const screen of SCREENS) {
    if (!screen.endsWith('.svelte')) continue;
    const source = readFileSync(new URL(screen, import.meta.url), 'utf8');
    for (const [, attribute, held] of source.matchAll(IN_THE_MARKUP)) {
      assert.ok(
        !/[A-Za-z]{2,}\s+[A-Za-z]{2,}/.test(held),
        `${screen} writes "${held.slice(0, 50)}" into ${attribute} instead of asking words.js ` +
          `for it. A shop that reads Bangla reads that in English.`,
      );
    }
  }
});

test('a shared module hands back a key, never a sentence', () => {
  // These files run behind both screens and cannot know which language the shop
  // reads, so a sentence built in one of them can only ever be English. Two
  // were: what is wrong with a file nobody can read, and why it is too early to
  // read one. A Bangla back office importing before it had synced read three
  // lines of English at the moment it was being told to wait.
  //
  // The scan above cannot see these, because it looks at the screens and these
  // are not screens. Same rule, other side of the boundary.
  for (const shared of SHARED) {
    const source = readFileSync(new URL(shared, import.meta.url), 'utf8');
    for (const found of source.matchAll(HANDED_BACK)) {
      const held = literalAt(source, found.index + found[0].length - 1);
      const prose = held.replace(/\$\{[^}]*\}/g, ' ');
      assert.ok(
        !/[A-Za-z]{2,}\s+[A-Za-z]{2,}/.test(prose),
        `${shared} hands back "${held.slice(0, 60)}" instead of a key. Whoever reads it reads ` +
          `English, whatever language the shop chose, and nothing else here would notice.`,
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
  const refusals = new Set([
    ...JSON.parse(readFileSync(new URL('./refusals.json', import.meta.url), 'utf8')),
    ...JSON.parse(readFileSync(new URL('./server_refusals.json', import.meta.url), 'utf8')),
  ]);
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
