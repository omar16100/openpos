import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { LANGUAGES, WORDS, paperWords, refusal, say } from './words.js';

const REFUSALS = JSON.parse(readFileSync(new URL('./refusals.json', import.meta.url), 'utf8'));
const PAPER = JSON.parse(readFileSync(new URL('./paper_words.json', import.meta.url), 'utf8'));

test('every refusal the till can give can be said in every language', () => {
  // The list is written out by a test in the core, from the codes the core
  // itself produces. A refusal this file cannot say is a cashier reading
  // English at the one moment it matters.
  for (const code of REFUSALS) {
    const held = WORDS[code];
    assert.ok(held, `${code} has no words at all: add it to words.js`);
    for (const { code: language } of LANGUAGES) {
      assert.ok(held[language], `${code} has no ${language}`);
    }
  }
});

test('every phrase exists in every language', () => {
  for (const [key, held] of Object.entries(WORDS)) {
    for (const { code: language } of LANGUAGES) {
      assert.ok(held[language], `${key} has no ${language}`);
    }
  }
});

test('a phrase in one language names the same things as in the others', () => {
  // A translation that dropped {name} leaves a cashier reading a sentence with
  // the shop's own figures missing from it, which is worse than English.
  const named = (phrase) => [...String(phrase).matchAll(/\{([a-z_]+)\}/g)].map((m) => m[1]).sort();
  for (const [key, held] of Object.entries(WORDS)) {
    const wanted = named(held.en);
    for (const { code: language } of LANGUAGES) {
      assert.deepEqual(named(held[language]), wanted, `${key} in ${language}`);
    }
  }
});

test('what is said comes back with the figures in it', () => {
  assert.equal(
    say('en', 'more-than-the-shelf-holds', { on_hand: '3 kg', name: 'Rice', wanted: '5 kg' }),
    'the shop has 3 kg Rice and this basket wants 5 kg',
  );
  assert.equal(
    say('bn', 'more-than-the-shelf-holds', { on_hand: '৩ kg', name: 'চাল', wanted: '৫ kg' }),
    'দোকানে চাল আছে ৩ kg, আর এই ঝুড়িতে চাওয়া হচ্ছে ৫ kg',
  );
});

test('a language that does not hold a phrase falls back to English', () => {
  // A screen older than the core it talks to says something imperfect rather
  // than nothing at all.
  assert.equal(say('bn', 'till.total'), 'মোট');
  assert.equal(say('xx', 'till.total'), 'Total');
});

test('a refusal nobody has translated yet still reads', () => {
  const view = {
    error: 'something the shop said that this build has never heard of',
    error_code: 'invented-tomorrow',
  };
  assert.equal(refusal('bn', view), view.error, 'the shop’s own sentence, rather than a code');

  // And one it knows is said in the language asked for, with its figures.
  const known = {
    error: 'wrong PIN: 2 tries left',
    error_code: 'wrong-pin',
    error_parts: { attempts_left: '2' },
  };
  assert.equal(refusal('bn', known), 'ভুল পিন: আর 2 বার চেষ্টা করা যাবে');
  assert.equal(refusal('en', known), 'wrong PIN: 2 tries left');
  assert.equal(refusal('en', {}), null, 'nothing refused, nothing said');
});

test('every label the core prints on paper can be said in every language', () => {
  // The list is written out by a test in the core, from the labels the core
  // itself asks for. A receipt in English beside a screen in Bangla is the
  // shop's own till disagreeing with its own paper.
  for (const key of PAPER) {
    const held = WORDS[`paper:${key}`];
    assert.ok(held, `paper:${key} has no words at all: add it to words.js`);
    for (const { code: language } of LANGUAGES) {
      assert.ok(held[language], `paper:${key} has no ${language}`);
    }
  }
});

test('the words a paper is given are only the ones asked for', () => {
  const said = paperWords('bn', ['receipt.total', 'nothing.like.this']);
  assert.equal(said['receipt.total'], 'সর্বমোট');
  assert.equal('nothing.like.this' in said, false);
  // English is the core's own default, so a language that says nothing changes
  // nothing and the paper reads as it always did.
  assert.deepEqual(paperWords('xx', PAPER), {});
});
