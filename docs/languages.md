# Languages: where the words live

**Purpose.** How openpos says anything to anybody, in the languages a shop reads.
**Status.** Current: the till and the back office speak English and Bangla.
**Last updated.** 8 September 2026.

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
| the server, a quarantine reason | the reason itself as postcard, turned into a name and its figures by the bindings, with the stored sentence beside it | the screen |
| the server, a tender on a sale looked up by receipt | which of the three kinds every shop has, beside the name a wallet was given | the screen |

## The tests that hold it together

Three, and they are what makes the arrangement survive a release:

1. `core/tests/refusal_codes.rs` freezes the list of refusal codes, refuses a code that is not on it
   and a list entry nothing can produce, and writes the list out to `apps/shared/refusals.json`.
2. `apps/shared/words.test.js` fails when a refusal in that file has no words in every language, and
   when a translation drops a figure the English names.
3. `core/tests/paper_words.rs` does the same for every label on a receipt, a drawer slip and an
   account page, writing `apps/shared/paper_words.json`; `words.test.js` checks the dictionary covers
   it.
4. `apps/shared/words.used.test.js` scans the screens: every key they ask for exists, and every key
   in the dictionary is asked for by something. A missing key falls back to English on purpose, so a
   typo would otherwise be invisible in the one place it matters.

## What a screen does

```js
import { refusal, say } from '../../shared/words.js';

const t = (key, fill) => say(language, key, fill);   // the screen's own words
fault = refusal(language, view);                     // whatever the till refused
```

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

## A word is not a figure

A gap in time arrives as a count, a unit and a direction. Passing all three into one sentence puts
"1 hours after" in the middle of a Bangla screen, which is how it read the first time it was walked.
The direction picks the sentence (`held.clock-after` and `held.clock-before`), and the unit picks a
phrase (`unit.hours`), so nothing crossing the boundary is an English word pretending to be a
figure.

## Open questions

- **Column alignment on a Bangla paper is approximate.** The layout pads by counting characters, and
  a Bangla conjunct or matra is more characters than it is columns wide. The figures still line up
  with each other because the padding is consistent, but a label's right edge can sit a place or two
  off. Doing it properly means grapheme clusters and a width table, which is a dependency this crate
  does not have.
- **The thermal path is English and stays English.** No ESC/POS code page carries Bangla, so
  `Command::Escpos` passes no words at all and gets the core's own defaults; `escpos::encode` already
  says which lines it could not print. A shop printing from a browser gets the language it chose.
- **A sale held before this was built can only be shown as its sentence.** The reason itself is
  stored beside the prose from now on (`sale.quarantine_kind`), and a row written before that column
  existed has nothing to translate against. The screen falls back to the words, which is what an
  operator read at the time anyway.
- **Adding a third language** is a column in `words.js` and an entry in `LANGUAGES`. The tests will
  name every phrase that is missing.
