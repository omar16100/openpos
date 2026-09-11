/// A message already on the screen follows the language, like every label.
///
/// The screens word a label every time they draw, so switching a till to Bangla
/// switches the whole screen. A message was different: it was worded once, at
/// the moment something went wrong, and then it sat there. So a cashier refused
/// in English who reached for the language switch to read the refusal watched
/// every label around the sentence change into Bangla and the sentence itself
/// stay in English. That is the one line on the screen they needed, and the only
/// line that would not follow, which reads like the switch is broken.
///
/// So a message holds its key and its figures and says itself when it is read.
import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { everyScreen } from './screens.js';
import { say, worded, wordedRefusal } from './words.js';

test('a message worded before the language changed is read in the language now', () => {
  let language = 'en';
  const said = worded(() => language, 'sync.idle');
  const inEnglish = String(said);
  language = 'bn';
  const inBangla = String(said);

  assert.equal(inEnglish, say('en', 'sync.idle'));
  assert.equal(inBangla, say('bn', 'sync.idle'));
  assert.notEqual(inBangla, inEnglish, 'this phrase is translated, so it must move');
});

test('the language is asked for when it is built as well as when it is read', () => {
  // Both matter, and for different reasons. Asking when it is read is what
  // makes a sentence already on the screen follow the switch. Asking when it is
  // built is what makes a screen notice: Svelte redraws what read something
  // that changed, and an attribute is written from this value rather than read
  // out of it, so a placeholder built without that read kept its English in a
  // Bangla shop. Seen on the till's discount box.
  let asked = 0;
  const said = worded(() => {
    asked += 1;
    return 'en';
  }, 'sync.idle');
  assert.equal(asked, 1, 'building it did not ask the language');
  String(said);
  assert.equal(asked, 2, 'reading it did not ask the language again');
});

test('the figures beside it are the ones it was given', () => {
  // The words follow the language and the numbers do not, which is the point:
  // the shop's own figures were worked out when the trouble happened.
  const said = worded(() => 'en', 'admin.vat_of', { rate: 12 });
  assert.ok(String(said).includes('12'), `no figure in: ${said}`);
});

test('a refusal nobody has translated still says what the till said', () => {
  const said = wordedRefusal(() => 'bn', {
    error: 'a sentence only this build knows',
    error_code: 'nothing.has.this.key',
  });
  assert.equal(String(said), 'a sentence only this build knows');
});

test('nothing refused is nothing to say', () => {
  assert.equal(wordedRefusal(() => 'en', { error: null }), null);
  assert.equal(wordedRefusal(() => 'en', null), null);
});

test('a refusal folded into another message is worded late as well', () => {
  // The import panel builds "line 4: <what the shop said>". Assembled as a
  // sentence it kept English inside a Bangla line, and the join is where that
  // happened: `say` turns whatever fills a brace into text, so the inner one is
  // asked at the same moment as the outer.
  let language = 'en';
  const inner = wordedRefusal(() => language, {
    error: 'refused',
    error_code: 'sync.idle',
  });
  const outer = worded(() => language, 'admin.refused_row', { line: 4, said: inner });
  const inEnglish = String(outer);
  language = 'bn';
  const inBangla = String(outer);

  assert.ok(inEnglish.includes(say('en', 'sync.idle')));
  assert.ok(inBangla.includes(say('bn', 'sync.idle')), `inner stayed put: ${inBangla}`);
  assert.ok(inBangla.includes('4'), 'the line number is a figure and does not move');
});


test('no screen words a message at the moment it happens', () => {
  // `say(language, ...)` is the eager form: it takes the language as it is now
  // and hands back a sentence. In markup that is harmless, because markup is
  // read again on every draw. Assigned to something the screen holds, it is the
  // defect above. The screens have no eager call left, and the way to keep it
  // that way is to have none at all: `t` and `refusal` are the deferred pair,
  // and every message goes through them.
  for (const { path, source } of everyScreen()) {
    const eager = [...source.matchAll(/\bsay\(\s*language\b/g)];
    assert.equal(
      eager.length,
      0,
      `${path} words something with the language as it ` +
        `is at that moment. Use t(key, fill), which words it when the screen reads it, so a ` +
        `message already on the screen follows the language switch like the labels around it do.`,
    );
  }
});

test('nothing decides what to show by reading the words', () => {
  // The till coloured its sync line by looking for "held up" in the sentence.
  // On a Bangla till the sentence never contains it, so the colour that says a
  // till has stopped reaching its shop only ever appeared in English.
  for (const { path, source } of everyScreen()) {
    const read = [
      ...source.matchAll(/\b(fault|done|syncing)\s*(?:\?\.)?\.(startsWith|includes|indexOf|match)\(/g),
    ];
    assert.equal(
      read.length,
      0,
      `${path} decides something by reading a message: ` +
        `${read.map((one) => one[0]).join(', ')}. What the sentence says depends on the shop's ` +
        `language, so the decision is right in English and wrong everywhere else. Keep a flag.`,
    );
  }
});
