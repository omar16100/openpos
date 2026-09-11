import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import {
  LANGUAGES,
  WORDS,
  languageNow,
  offeredLanguages,
  paperWords,
  refusal,
  say,
} from './words.js';

const REFUSALS = JSON.parse(readFileSync(new URL('./refusals.json', import.meta.url), 'utf8'));
const PAPER = JSON.parse(readFileSync(new URL('./paper_words.json', import.meta.url), 'utf8'));
const FROM_THE_SERVER = JSON.parse(
  readFileSync(new URL('./server_refusals.json', import.meta.url), 'utf8'),
);
const TRAIL = JSON.parse(readFileSync(new URL('./trail_codes.json', import.meta.url), 'utf8'));

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

test('every refusal the shop’s server gives can be said in every language', () => {
  // The server was the last place here that could only speak English, and the
  // refusals it gives are the ones an owner has to act on: a save built on a
  // stale copy, a barcode another item holds, an item the shop has traded.
  for (const code of FROM_THE_SERVER) {
    const held = WORDS[code];
    assert.ok(held, `${code} has no words at all: add it to words.js`);
    for (const { code: language } of LANGUAGES) {
      assert.ok(held[language], `${code} has no ${language}`);
    }
  }
});

test('a refusal from the till and one from the server never share a name', () => {
  // One dictionary serves both lists, so a name used twice would give one set
  // of words to two different refusals. The core freezes this too; it is
  // checked here as well because this is the file that would be wrong.
  for (const code of FROM_THE_SERVER) {
    assert.equal(REFUSALS.includes(code), false, `${code} is in both frozen lists`);
  }
});

test('every number a trail can hold has a phrase in every language', () => {
  // What an owner reads when they ask what happened at a counter that evening.
  // These are asked for by number rather than by name, so the test that scans
  // the screens for keys cannot see them: action twelve, a sale to somebody
  // already past what they may owe, read as English in a Bangla shop from the
  // day it was added, and nothing anywhere said so.
  for (const code of TRAIL) {
    const held = WORDS[`allowed.${code}`];
    assert.ok(held, `allowed.${code} has no words at all: add it to words.js`);
    for (const { code: language } of LANGUAGES) {
      assert.ok(held[language], `allowed.${code} has no ${language}`);
    }
  }
});

test('the three tender kinds every shop has can all be said', () => {
  // Built at run time from what the core calls them, so the scan that checks
  // which keys a screen asks for cannot see them, and the scan that checks the
  // dictionary is not wasted explicitly skips them. Both holes in one place:
  // `till.credit` was missing and a drawer report with a sale on account in it
  // read "till.credit (not in the till)" on the screen a shop counts its money
  // against.
  //
  // A wallet is deliberately not here. It keeps the name the shop gave it,
  // because "bKash" is a name and not a word to translate.
  for (const kind of ['cash', 'card', 'credit']) {
    const held = WORDS[`till.${kind}`];
    assert.ok(held, `till.${kind} has no words at all: a screen would show the key itself`);
    for (const { code: language } of LANGUAGES) {
      assert.ok(held[language], `till.${kind} has no ${language}`);
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

test('a phrase that promises a time is promising what the till actually takes', () => {
  // Five messages told a shopkeeper that a change reaches the counter "within
  // ten minutes". They were written when the lists were re-read on that
  // cadence, and they stayed there after the shop's settings number arrived,
  // which is checked every thirty seconds and pulls the lists in again the
  // moment it has moved. Measured on a real till: a stock rule saved in the
  // back office refused a scan at the counter nine seconds later.
  //
  // Twenty times wrong in the shop's favour is still wrong. An owner who
  // suspends somebody and is told to wait ten minutes walks away, and what
  // they learn is not to believe the screen.
  const driver = readFileSync(new URL('../../core/src/sync/driver.rs', import.meta.url), 'utf8');
  const idle = driver.match(/pub const IDLE_MS: u64 = (\d+) \* 1_000;/);
  assert.ok(idle, 'the cadence the promise is about is no longer written that way');
  const seconds = Number(idle[1]);

  const promises = Object.entries(WORDS).filter(([, said]) => /half a minute/.test(said.en ?? ''));
  assert.ok(promises.length > 0, 'nothing promises a time any more: take this test out with it');
  assert.ok(
    seconds <= 30,
    `${promises.length} phrases promise a shopkeeper that a change reaches the till within half a ` +
      `minute, and a till now looks every ${seconds} seconds: ${promises
        .map(([key]) => key)
        .join(', ')}. Change the words with the cadence, in the same commit.`,
  );
});

test('a shop that has said nothing offers every language there is', () => {
  assert.deepEqual(offeredLanguages(undefined), LANGUAGES);
  assert.deepEqual(offeredLanguages([]), LANGUAGES);
  assert.deepEqual(offeredLanguages(null), LANGUAGES);
});

test('a shop that offers one language offers only that one', () => {
  assert.deepEqual(
    offeredLanguages(['en']).map((one) => one.code),
    ['en']
  );
  assert.deepEqual(
    offeredLanguages(['bn']).map((one) => one.code),
    ['bn'],
    'and a shop where nobody reads English is as real as the other way round'
  );
  assert.deepEqual(
    offeredLanguages(['BN', 'bn']).map((one) => one.code),
    ['bn'],
    'said twice, in two spellings, is still one language'
  );
});

test('a list of languages this build has never heard of leaves a screen with words', () => {
  // A shop upgraded its server and not its tablets, or somebody typed a code
  // into a settings field. A screen with no words on it is worse than a screen
  // in the wrong ones, so nothing said is what this means.
  assert.deepEqual(offeredLanguages(['fr', 'ur']), LANGUAGES);
});

test('a device left in a language the shop has since turned off is brought back', () => {
  // The device this setting exists for, and the one a naive version strands: a
  // till somebody switched to Bangla, in a shop that then decides it works in
  // English. Gating only the button that switches would leave this device in
  // Bangla with the way out removed.
  assert.equal(languageNow('bn', ['en']), 'en');
  assert.equal(languageNow('en', ['bn']), 'bn', 'and the same the other way');
});

test('a device is left in the language it was in when the shop still offers it', () => {
  assert.equal(languageNow('bn', ['en', 'bn']), 'bn');
  assert.equal(languageNow('bn', []), 'bn', 'a shop that has said nothing offers it');
  assert.equal(languageNow('en', undefined), 'en');
});

test('a device that remembers nothing gets the first language the shop offers', () => {
  assert.equal(languageNow(null, ['bn']), 'bn');
  assert.equal(languageNow('', ['en', 'bn']), 'en');
  assert.equal(languageNow(undefined, undefined), 'en');
});

test('no English phrase carries a word of Bangla', () => {
  // A shop that has set itself to English reads no Bangla anywhere, and that
  // has to hold for the words this product chose as much as for the words the
  // shop typed. Two of these existed, both in the setting that turns Bangla
  // off: the options named each language in itself, so the screen that says
  // "English only" said it in two scripts.
  //
  // Scanned rather than trusted, because the tempting way to write a phrase
  // about a language is in that language, and the one screen where it is most
  // tempting is the one where it is most wrong.
  const bengali = /[ঀ-৿]/;
  const carrying = Object.entries(WORDS)
    .filter(([, said]) => typeof said.en === 'string' && bengali.test(said.en))
    .map(([key]) => key);
  assert.deepEqual(
    carrying,
    [],
    'an English screen must be English: name the other language in English here'
  );
});

test('every phrase this product says has both languages', () => {
  // The other direction of the same rule, and the cheaper failure: a phrase
  // with no Bangla falls back to English, so a shop reading Bangla finds an
  // English sentence in the middle of its screen.
  const missing = Object.entries(WORDS)
    .filter(([, said]) => typeof said.en === 'string' && typeof said.bn !== 'string')
    .map(([key]) => key);
  assert.deepEqual(missing, [], 'these are said in English on a Bangla screen');
});
