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
import { EVERY_STORAGE_TROUBLE, WHAT_ELSE_TO_TRY } from './storage_trouble.js';
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
const BUILT_AT_RUN_TIME =
  /^(till\.(cash|card|credit|storage_)|file\.|allowed\.|held\.|unit\.|paper:)/;

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

/// The message slots a screen puts a sentence into for somebody to read, and
/// the second argument of `attempt`, which is what a screen says after
/// something worked.
///
/// The whole statement is read rather than the character after the `=`. It used
/// to be the character after the `=`, so a sentence behind a ternary was
/// invisible: `fault = writtenOff ? 'say how much to strike off' : ...` sat in
/// the back office in English, in a shop that had chosen Bangla, with a test
/// standing over it saying no screen says a sentence of its own.
const SAID_TO_SOMEBODY = /\b(fault|done|note)\s*=|attempt\(/g;

/// The shared modules, which run on both screens and know no language at all.
const SHARED = ['./catalogue_file.js', './till.js', './counting.js', './buying.js'];

/// A sentence leaving a module or a screen: returned, or handed back as a
/// fault. Screens are scanned for this too, because a screen that builds a
/// sentence in a function and returns it has the same problem as one that
/// assigns it: "the shop has -9, this wants 1" sat under a basket line, in
/// English, in a shop that had chosen Bangla.
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

/// Every string literal in the statement that starts at `at`.
///
/// Read to the end of the statement rather than to the end of the line, because
/// a choice between two sentences is written across three of them. Depth is
/// tracked so a `;` inside a call or an object does not end it early.
function literalsIn(source, at) {
  const held = [];
  let depth = 0;
  let index = at;
  while (index < source.length) {
    const here = source[index];
    // Comments first. An apostrophe in one ("the core's own English") reads as
    // the start of a string otherwise, and everything after it is nonsense.
    if (here === '/' && source[index + 1] === '/') {
      index = source.indexOf('\n', index);
      if (index < 0) return held;
      continue;
    }
    if (here === '/' && source[index + 1] === '*') {
      const ends = source.indexOf('*/', index);
      if (ends < 0) return held;
      index = ends + 2;
      continue;
    }
    if (here === '(' || here === '[' || here === '{') depth += 1;
    else if (here === ')' || here === ']' || here === '}') {
      depth -= 1;
      if (depth < 0) return held;
    } else if (here === "'" || here === '"' || here === '`') {
      const said = literalAt(source, index);
      // Only what this statement says itself. A literal nested inside a call
      // is that call's business: the key handed to `t()`, or the name of a
      // field in a request. Reading those too would fail on `what:
      // 'amend_operator'` and teach whoever hit it to work around this test.
      if (depth === 0) held.push(said);
      index += said.length + 2;
      continue;
    } else if (here === ';' && depth === 0) return held;
    else if (here === '\n' && depth === 0 && /[;{}]\s*$/.test(source.slice(at, index))) {
      return held;
    }
    index += 1;
  }
  return held;
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
      for (const held of literalsIn(source, found.index + found[0].length)) {
        const prose = held.replace(/\$\{[^}]*\}/g, ' ');
        assert.ok(
          !/[A-Za-z]{2,}\s+[A-Za-z]{2,}/.test(prose),
          `${screen} says "${held.slice(0, 60)}" itself instead of asking words.js for it. A ` +
            `shop that reads Bangla would read that line in English, and no other test here ` +
            `would notice.`,
        );
      }
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

test('no screen writes English into the markup itself', () => {
  // The third place a sentence can hide, and the last one either scan above
  // could not see: plain text between tags. "Back to scanning", "Take them in",
  // "That is not a bundle. Check the whole of it was copied." Nine of them, read
  // in English by every shop that chose Bangla.
  //
  // Comments and expressions are taken out first. What is left is what somebody
  // standing at the counter reads.
  for (const screen of SCREENS) {
    if (!screen.endsWith('.svelte')) continue;
    const source = readFileSync(new URL(screen, import.meta.url), 'utf8');
    let markup = source.split('</script>')[1] ?? '';
    markup = markup.split('<style>')[0];
    markup = markup.replace(/<!--[\s\S]*?-->/g, ' ');
    // Braces nest, so this runs until it stops finding any.
    for (let pass = 0; pass < 4; pass += 1) markup = markup.replace(/\{[^{}]*\}/g, ' ');
    for (const [, between] of markup.matchAll(/>([^<>]*)</g)) {
      const prose = between.replace(/&[a-z]+;/g, ' ');
      assert.ok(
        !/[A-Za-z]{2,}\s+[A-Za-z]{2,}/.test(prose),
        `${screen} has "${prose.trim().replace(/\s+/g, ' ').slice(0, 60)}" written into the ` +
          `markup instead of asking words.js for it. A shop that reads Bangla reads it in English.`,
      );
      // And one word is enough. Most of what somebody presses is one word, and
      // the two-word rule read straight past a button that said "Look" on a
      // screen where every other button had turned over into Bangla. The
      // shop's own name is not a word anybody translates, so it is allowed.
      const word = prose.trim().replace(/\s+/g, ' ');
      assert.ok(
        !/^[A-Za-z]{2,}$/.test(word) || word === 'openpos',
        `${screen} has the button or label "${word}" written into the markup instead of asking ` +
          `words.js for it. One word is what most of them are, and a shop that reads Bangla ` +
          `reads it in English.`,
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
  for (const shared of [...SHARED, ...SCREENS]) {
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
  // because they are asked for by code rather than by name. The storage
  // troubles are the same arrangement with a shorter journey: they are born in
  // JavaScript, so the list comes from the module that names them and its own
  // test holds the dictionary to it.
  const refusals = new Set([
    ...JSON.parse(readFileSync(new URL('./refusals.json', import.meta.url), 'utf8')),
    ...JSON.parse(readFileSync(new URL('./server_refusals.json', import.meta.url), 'utf8')),
    ...EVERY_STORAGE_TROUBLE,
    ...WHAT_ELSE_TO_TRY,
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
