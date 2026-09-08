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
| `core`, a sale's receipt | the finished lines | nobody: the receipt is still English (see below) |
| `bindings`, the sync line | a key and its figures | the screen |
| `bindings`, the trail of what was allowed | the number the till stored, plus an English sentence as a fallback | the screen |
| `apps/shared/catalogue_file.js`, a row that cannot be written | a code and its figures | the screen |
| the server, a quarantine reason | the sentence, in English | nobody yet: see open questions |

## The tests that hold it together

Three, and they are what makes the arrangement survive a release:

1. `core/tests/refusal_codes.rs` freezes the list of refusal codes, refuses a code that is not on it
   and a list entry nothing can produce, and writes the list out to `apps/shared/refusals.json`.
2. `apps/shared/words.test.js` fails when a refusal in that file has no words in every language, and
   when a translation drops a figure the English names.
3. `apps/shared/words.used.test.js` scans the screens: every key they ask for exists, and every key
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

## Open questions

- **The receipt is still English.** Its words are built in `core/src/receipt/`, and the way to do it
  is the way the refusals went: the caller supplies the words and the core holds none, so the ESC/POS
  path keeps English while a browser-printed one can be either. Thermal paper cannot render Bangla at
  all without a raster path, which is its own open item, so today this only reaches a shop printing
  from a browser.
- **The server's quarantine reasons are still English sentences.** They are stored as text in the
  database, so old rows can only ever be shown as they were written; a code beside the text would let
  new ones be translated. Both would have to travel.
- **Adding a third language** is a column in `words.js` and an entry in `LANGUAGES`. The tests will
  name every phrase that is missing.
