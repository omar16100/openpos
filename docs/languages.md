# Languages: where the words live

**Purpose.** How openpos says anything to anybody, in the languages a shop reads.
**Status.** Current: the till and the back office speak English and Bangla.
**Last updated.** 13 September 2026.

## What it is for

The first market is Bangladesh, and a cashier at a counter in a mofussil town does not read English.
Until this was built, every word on every screen was English and the only Bangla anywhere was the
item names a shop had typed in itself. `docs/06092026_openpos_feature_spec.md` has carried "i18n,
English and Bangla" in v1 since it was written.

## The rule

**Words that a person reads live in one file: `apps/shared/words.js`.** Everything else carries a
name for what happened and the figures beside it.

That is the whole design, and it exists because of what the alternative costs. A refusal is worded
where it is decided: in the core, in Rust, in English. A screen that wanted to say it in Bangla could
only match on the English sentence, and would go quiet the day somebody improved the wording. Worse,
half of those sentences carry the shop's own figures ("the shop has 3 kg Rice and this basket wants
5 kg"), and a sentence cannot be translated with the numbers baked into it.

So:

| Where it is decided | What crosses the boundary | Who says it |
|---|---|---|
| `core`, a refusal | `error_code` (frozen) and `error_parts` (named, already formatted) | the screen, from `words.js` |
| `core`, a paper (receipt, drawer slip, account page) | the finished lines, laid out with the words the caller supplied | the screen, which hands the core its words |
| `bindings`, the sync line | a key and its figures | the screen |
| `bindings`, the trail of what was allowed | the number the till stored, plus an English sentence as a fallback | the screen |
| `apps/shared/catalogue_file.js`, a row that cannot be written | a code and its figures | the screen |
| the server, a refusal | the code, its figures named, and the English sentence as the fallback | the screen |
| the server, a quarantine reason | the reason itself as postcard, turned into a name and its figures by the bindings, with the stored sentence beside it | the screen |
| the server, a tender on a sale looked up by receipt | which of the three kinds every shop has, beside the name a wallet was given | the screen |

## The tests that hold it together

Six, and they are what makes the arrangement survive a release:

1. `core/tests/refusal_codes.rs` freezes the list of refusal codes, refuses a code that is not on it
   and a list entry nothing can produce, and writes the list out to `apps/shared/refusals.json`. It
   does the same for the refusals the server gives, into `apps/shared/server_refusals.json`, and
   fails if a name appears in both lists.
2. `apps/shared/words.test.js` fails when a refusal in that file has no words in every language, and
   when a translation drops a figure the English names. It also holds the plural rule above: no
   phrase says "(s)", no language but English offers a choice between words, and no English phrase
   puts a figure straight before a noun that only knows how to be many.
3. `core/tests/paper_words.rs` does the same for every label on a receipt, a drawer slip and an
   account page, writing `apps/shared/paper_words.json`; `words.test.js` checks the dictionary covers
   it.
4. `apps/shared/words.used.test.js` scans the screens: every key they ask for exists, and every key
   in the dictionary is asked for by something. A missing key falls back to English on purpose, so a
   typo would otherwise be invisible in the one place it matters.
5. The same file's third test scans for a screen that skips the dictionary and assigns English
   straight to the line somebody reads. Five sentences survived the first pass that way, including
   the count at the end of an import, because both tests above check keys and neither notices a
   screen that asks for no key at all. Two English words in a row in a `fault`, `done` or `note`
   assignment fails it.
6. `apps/shared/worded_late.test.js` holds the deferred wording to what it is for: a message worded
   before the language changed reads in the language now, a refusal folded into another message
   follows too, no screen words anything with the language as it is at that moment, and nothing
   decides what to show by reading a message's words.

## Which languages a shop offers

A shop says which of them it offers its own staff, and the setting sits with the shop's details
beside the wallets and the stock rule. Empty means every language the device has, which is what
every shop meant before the field existed, so no shop was changed by its arrival.

It is there because a shop is not this product's idea of a shop. One where nobody reads English does
not want a button on the till that can put a cashier into it; one that works in English does not
want that button either. Both is right for the shop in between, and that is the default.

**A screen drawn in English shows no Bangla, and a shop's own words are never touched.** Those are
two rules, and the difference between them is the whole of it.

What is not rendered on an English screen: any Bangla, including a shop's own Bangla item name,
which used to be shown beside the English one on every row of the lookup list and on the price
check. On a till set to English that second line is a script nobody at that counter reads, on every
row a cashier reads at speed with a customer waiting. The back office's box for typing a Bangla name
is drawn only when the shop offers Bangla, for the same reason: it is a field those staff cannot
use.

What is never touched: the words themselves. A Bangla name is still stored, still travels, still
what a Bangla search matches on, still on the item when the shop offers Bangla again, and still
shown in full on a Bangla screen. Nothing is deleted and nothing becomes unreachable, because the
setting is reversible and the data outlives it. A setting that deleted a shop's own words in the
name of a preference about menus would be a different and much worse thing.

A guard holds the first rule for this product's own words: no English phrase in `words.js` may carry
a word of Bangla. Two did when it was written, both in the setting that turns Bangla off, because
the tempting way to write a phrase about a language is in that language and the screen that says
"English only" was saying it in two scripts.

Two rules, and the second is the one that matters:

```js
import { languageNow, offeredLanguages } from '../../shared/words.js';

const language = $derived(languageNow(remembered, view?.languages));
const offered = $derived(offeredLanguages(view?.languages));
```

The language is decided against what the shop offers **on every draw**, not when the button that
switches is drawn. Gating the button is the obvious version and it strands the one device the
setting exists for: a till somebody had already left in Bangla, in a shop that then turns Bangla
off, would sit in it with the way out removed. What the device remembers is never overwritten, so a
shop that turns Bangla back on returns that till to the language it was in, with nothing pressed.

A list naming only codes a build has never heard of counts as nothing said, because a screen with no
words on it is worse than a screen in the wrong ones. Walked both ways against a live server: the
till came back to English by itself, with `bn` still in its own storage, and returned to Bangla when
the shop offered it again.

## What a screen does

```js
import { worded, wordedRefusal } from '../../shared/words.js';

const t = (key, fill, otherwise) => worded(() => language, key, fill, otherwise);
const refusal = (view) => wordedRefusal(() => language, view);

fault = t('till.nothing_to_park');   // the screen's own words
fault = refusal(view);              // whatever the till refused
```

Both hand back something that holds the key and the figures and says itself when it is read, which
is when the screen draws. That is the difference between a label and a message. A label is worded
again on every draw, so it follows the language. A message used to be worded once, at the moment
something went wrong, and then it sat there: a cashier refused in English who switched to Bangla to
read it watched every label around the sentence change and the sentence stay. Now both follow.

Because `say` turns whatever fills a brace into text, a message folded inside another message is
worded late as well. The import panel's "line 4: `<what the shop said>`" is one sentence built from
two, and both halves follow the switch.

Two rules come with it, and both were broken before anyone noticed:

- **Never decide anything by reading a message.** The till coloured its sync line by looking for the
  words "held up" in the sentence, so on a Bangla till the colour that says a till has stopped
  reaching its shop never appeared. Keep a flag beside the message.
- **Never write the code's own word onto the screen.** `storage` is `opfs`, `memory`, `opening`,
  `unavailable`; `syncing` started as `idle`. Printed as they stand they are English in a Bangla
  shop. They go through the dictionary by their code, `till.storage_unavailable` and the rest, with
  the code itself as the fallback for a state this build has not been taught to say.

`say(language, ...)` still exists for anything outside a screen, and `apps/shared/worded_late.test.js`
fails if a screen calls it: from there it is always the eager form.

One more thing the deferred form needs, and it is not obvious. `worded` asks for the language when it
is built as well as when it is read. Asking when it is read is what makes a sentence already on the
screen follow the switch. Asking when it is built is what makes the screen notice at all: Svelte
redraws what read something that changed, and an attribute is written from this value rather than
read out of it, so a `placeholder={t(...)}` built without that read keeps the language it was first
drawn in. The till's ticket discount box sat in English on a Bangla screen for exactly that reason.

## What a phrase may promise

A phrase that tells a shopkeeper how long something takes is a claim about a constant in the core.
Five said "within ten minutes", which was the cadence the people and the shop's own details are
re-read on; the shop's settings number, checked every thirty seconds, had made that twenty times too
slow. They say half a minute now, and `words.test.js` reads `IDLE_MS` out of `core/src/sync/driver.rs`
and fails if the cadence moves past what the words promise.

`language` is per device and per app, held in `localStorage` under `openpos.language` and
`openpos.admin.language`. Two apps share an origin, and a shopkeeper may well want the counter in
Bangla and the back office in English.

Anything with no entry falls back to English, and a refusal with no entry falls back to the sentence
the core sent. A screen older than the core it talks to says something imperfect rather than nothing.

## What will surprise you

- **Digits stay Western.** 212.75, not ২১২.৭৫. That is what most Bangladeshi shop screens use, and it
  keeps a price the same shape on the screen, on the paper and in the spreadsheet a shop exports.
- **The shop's own words are never translated.** Item names, till labels, supplier names, wallet
  names: "bKash" is a name, not a word. The three tender kinds every shop has (cash, card, on
  account) are said in the shop's language, and a wallet keeps the name the shop gave it.
- **The Bangla has not been read by a native speaker.** It is written to be read by a shopkeeper
  rather than to be literary, and it is worth a pass by somebody who speaks it before a shop sees it.

## The refusals the server gives

These were the last words here that could only be English, and they are the ones
an owner has to act on: a save built on a stale copy, a barcode another item
already holds, an item the shop has traded, a rate no till could price.

They now take the same shape as everything else. `ProtocolError::code()` names
the refusal, `refusalNamed` in the bindings decodes the body the server sent and
gives back the code, its figures already formatted, and the English sentence
beside them. The worker hangs those on the error it throws; the bridge sends them
as their own fields, because an `Error` does not survive a `postMessage` with
anything hung on it; and the screen words it with `refusal(language, …)`, the
same call the till already used for its own refusals.

Two of them are named apart from the till's refusal of the same shape on
purpose. A till refusing "not permitted" is a cashier who may not do that; the
server refusing it is a device that may not, and one set of words serving both
would send a shopkeeper looking for a supervisor when the fix is a different
device. `core/tests/refusal_codes.rs` fails if the two frozen lists ever share a
name, and `apps/shared/words.test.js` checks the same thing from the other side.

| Where it is decided | What crosses the boundary |
|---|---|
| `core`, a `ProtocolError` | the code, the figures, and the English sentence |
| `bindings`, `refusalNamed` | the three of them as JSON |
| `apps/shared/till.worker.js` | `error_code` and `error_parts` beside `error` |
| the screen | `refusal(language, …)` |

## The one figure that is still an English clause

`ProtocolError::NotAPrice { said }` carries a sentence, not a figure: "150
percent is not a tax rate", "a price of 12.00 is below nothing". The screen puts
the shop's own words around it, so a Bangla back office reads a Bangla sentence
with an English clause inside it.

Doing it properly means splitting that variant into the three refusals it
actually is, each with its own figure. `ProtocolError` is encoded positionally,
`NotAPrice` is variant nine, and the protocol went from 2 to 3 the same day this
was written: changing what index nine means would be the second change to that
shape in one session, and a back office built against the first would decode the
second as the wrong refusal with conviction. It is worth doing at the next
protocol bump, and it is not worth doing on its own.

## A word is not a figure

A gap in time arrives as a count, a unit and a direction. Passing all three into one sentence puts
"1 hours after" in the middle of a Bangla screen, which is how it read the first time it was walked.
The direction picks the sentence (`held.clock-after` and `held.clock-before`), and the unit picks a
phrase (`unit.hours`), so nothing crossing the boundary is an English word pretending to be a
figure.

## One is not two, and only English cares

A brace holding a slash is a choice rather than a figure: what to say when the
count is one, and what to say when it is not. `{/s}` is the plural s, `{is/are}`
is the verb that has to agree with it, and `{f/ves}` and `{y/ies}` are the stems
that do not simply take an s.

```js
'admin.sales_of': { en: '{count} sale{/s}', bn: '{count} টি বিক্রি' },
'admin.shelves_entered': { en: '{count} shel{f/ves} entered', bn: '{count} টি তাক লেখা হয়েছে' },
```

The count is `count` unless the brace names another, which is `{ready:/s}` on
the one phrase whose number is called something else. Anything that is not
exactly one takes the plural, including none and including a count nobody
passed: "0 sales" is right, and a missing number is likelier to be many than
one.

**Written in the English phrase and nowhere else.** English inflects for number
and Bangla does not, so a Bangla phrase carrying one of these would be somebody
translating an English grammar rule into a language with no use for it. It is
the one place a rule of grammar lives inside a phrase rather than beside it, and
it is here because the alternative is two keys and a caller that picks between
them, which is what the units behind "rung 3 hours after" still do and what
nobody wants to write twice for every noun in the shop.

What it replaced was twenty eight phrases reading "1 sale(s)" and a dozen more
that wrote the plural into the word itself, so "1 shelves entered" and "1 tries
left" and "wait 1 seconds". A count of one is the ordinary case on most of these
screens.

Three tests in `words.test.js` hold it: no phrase in any language may say "(s)",
no language but English may offer a choice, and no English phrase may put a
figure straight before a noun that only knows how to be many. The nouns for that
last one are written out rather than guessed at, because guessing means a rule
about words ending in s and this dictionary is full of verbs that end in s.

## Open questions

- **A Bangla paper reads ragged on a screen, and that is a consequence of one layout serving two
  things.** The papers are laid out for a fixed-width printer: labels are padded by counting
  characters so an amount lands in the same column on every line. Bangla defeats that twice over. A
  matra or a hasant is its own character and draws no column of its own, and almost no machine has a
  monospace Bangla font, so what the browser draws is proportional whatever the count says.

  Counting columns rather than characters was tried and reverted: it makes the count right and the
  screen no better, because the width is the font's and not the string's. The two real answers are a
  screen that lays the receipt out itself with the amounts right-aligned, which is a second
  implementation of the layout and the thing `receipt::Line` exists to prevent, or a raster path that
  draws the paper as an image, which is what Bangla on a thermal printer needs anyway. Neither is
  built. On paper, where it matters today, the printer prints English and the columns are right.
- **All paper is English, whatever the screen says.** Three reasons pointing the same way: no
  ESC/POS code page carries Bangla, so a thermal printer gets English regardless; the layout pads by
  counting characters, which Bangla defeats, so a Bangla slip comes out ragged; and a shop with two
  languages on its counter should not keep two shapes of receipt in its records. `paperWords` has no
  caller now. The mechanism stays, because it is what makes paper translatable at all and the raster
  path will want it, and the words are still checked to exist in both languages so they cannot rot
  while they wait.
- **The thermal path is English and stays English.** No ESC/POS code page carries Bangla, so
  `Command::Escpos` passes no words at all and gets the core's own defaults; `escpos::encode` already
  says which lines it could not print. A shop printing from a browser gets the language it chose.
- **A sale held before this was built can only be shown as its sentence.** The reason itself is
  stored beside the prose from now on (`sale.quarantine_kind`), and a row written before that column
  existed has nothing to translate against. The screen falls back to the words, which is what an
  operator read at the time anyway.
- **Adding a third language** is a column in `words.js` and an entry in `LANGUAGES`. The tests will
  name every phrase that is missing.
