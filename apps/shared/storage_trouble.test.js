import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { everyScreen } from './screens.js';
import { LANGUAGES, WORDS, say } from './words.js';
import {
  EVERY_STORAGE_TROUBLE,
  WHAT_ELSE_TO_TRY,
  alreadyOpenHere,
  storageTrouble,
  whatElseToTry,
  whyStorageFailed,
} from './storage_trouble.js';

/// What Chrome actually threw, copied from a session where it happened.
///
/// Two till tabs on one device: the second one showed this sentence at the top
/// of the screen and an enrolment box underneath it.
const CHROME = Object.assign(
  new Error(
    "Failed to execute 'createSyncAccessHandle' on 'FileSystemFileHandle': Access Handles " +
      'cannot be created if there is another open Access Handle or Writable stream associated ' +
      'with the same file.',
  ),
  { name: 'NoModificationAllowedError' },
);

test('a till open in another tab is named as that and nothing else', () => {
  assert.equal(whyStorageFailed(CHROME), 'till-open-elsewhere');
  assert.equal(alreadyOpenHere(whyStorageFailed(CHROME)), true);
});

test('the name is enough, and so is the sentence', () => {
  // Either half on its own. Safari has used a different name for the same
  // thing, and a browser that changes its wording must not turn this back into
  // an invitation to enrol.
  assert.equal(whyStorageFailed({ name: 'InvalidStateError', message: 'no idea' }), 'till-open-elsewhere');
  assert.equal(
    whyStorageFailed(new Error('Access Handles cannot be created for this file')),
    'till-open-elsewhere',
  );
});

test('a full device and a browser that keeps nothing are told apart', () => {
  // Three different things a shop does: close the other tab, delete something,
  // or accept that this browser forgets. One message for all three would be
  // three shops doing the wrong thing.
  assert.equal(
    whyStorageFailed(Object.assign(new Error('out of space'), { name: 'QuotaExceededError' })),
    'no-room-on-this-device',
  );
  assert.equal(
    whyStorageFailed(Object.assign(new Error('nope'), { name: 'SecurityError' })),
    'this-browser-keeps-nothing',
  );
  assert.equal(alreadyOpenHere('no-room-on-this-device'), false);
});

test('a failure nobody has seen keeps the browser’s own sentence', () => {
  // Null rather than a guess: the screen falls back to what the browser said,
  // which is worth more to whoever is sent to look at it than a wrong name.
  assert.equal(whyStorageFailed(new Error('something else entirely')), null);
  assert.equal(whyStorageFailed(null), null);
  assert.equal(whyStorageFailed(undefined), null);
  assert.equal(alreadyOpenHere(null), false);
});

test('the browser’s own exception is never written to', () => {
  // What the browser throws is a DOMException, whose `code` is a read-only
  // getter from an older standard. Hanging the reason on it inside a module
  // throws a TypeError, so the name meant for the screen would become a second
  // failure thrown from the handler for the first. Modelled exactly.
  const browsers = Object.defineProperty(
    new Error('Access Handles cannot be created if there is another open Access Handle'),
    'code',
    { get: () => 0, configurable: false },
  );
  assert.throws(
    () => {
      'use strict';
      browsers.code = 'till-open-elsewhere';
    },
    TypeError,
    'the model is only worth anything if writing to it really does throw',
  );

  const carried = storageTrouble(browsers);
  assert.equal(carried.code, 'till-open-elsewhere');
  assert.match(carried.message, /Access Handles/, 'and the browser’s own sentence survives');
  assert.equal(carried.cause, browsers, 'with the original underneath it for whoever looks');
});

test('every reason this file can name has words for a shopkeeper', () => {
  // The dictionary is checked against this list rather than the other way
  // round, because this file is where a code is born. A code with no words is
  // a shopkeeper reading "till-open-elsewhere" off a screen.
  //
  // Asked of the dictionary itself rather than of the text of the file: a
  // scan for the code would pass on a code that appears only in a comment,
  // which is exactly the shape of a phrase somebody meant to add and did not.
  for (const code of EVERY_STORAGE_TROUBLE) {
    assert.ok(WORDS[code], `${code} can be shown to somebody and words.js has no phrase for it`);
    assert.ok(WORDS[code].en, `${code} has no English, which is what every other language falls back to`);
    assert.notEqual(
      say('bn', code),
      WORDS[code].en,
      `${code} is shown in English to a shop that chose Bangla`,
    );
  }
});

test('no screen decides any of this for itself', () => {
  // The rule lives here so both the till and the back office get the same
  // answer, and so does anything else that opens a store later. A screen
  // matching on the browser's sentence is a screen that stops matching the day
  // the browser rewords it, or the day the shop switches to Bangla.
  for (const { path, source } of everyScreen()) {
    assert.equal(
      /Access Handles|createSyncAccessHandle|NoModificationAllowedError/.test(source),
      false,
      `${path} is matching on a browser’s own wording instead of asking storage_trouble.js`,
    );
  }
});

test('the first advice is offered once, and then the second one', () => {
  // "Close the other window" is right about the case it was written for and is
  // a dead end when there is no other window: a browser goes on holding a
  // shop's ledger for a window that has already gone, and then the only way out
  // the screen offers has already been taken. Met three times in one day of
  // walking, each time with one tab open in the whole browser.
  assert.equal(whatElseToTry(0), null, 'one instruction at a time');
  assert.equal(whatElseToTry(1), WHAT_ELSE_TO_TRY[0]);
  assert.equal(whatElseToTry(9), WHAT_ELSE_TO_TRY[0], 'and it does not change again');
});

test('what else to try can be said in every language', () => {
  // Built at run time, so the scan of the screens cannot see it and nobody
  // would notice it was never translated: the screen would show the key.
  for (const key of WHAT_ELSE_TO_TRY) {
    const held = WORDS[key];
    assert.ok(held, `${key} is what a screen offers and words.js does not hold it`);
    for (const { code } of LANGUAGES) assert.ok(held[code], `${key} has no ${code}`);
  }
});
