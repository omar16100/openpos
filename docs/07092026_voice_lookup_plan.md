# Plan: a cashier says what they want, and it comes up

Purpose: add a third way onto a ticket, for a shop where one hand is holding the goods.
Status: phases 0 and 1 done and evidenced; phase 2 next.
Last updated: 2026-09-07.

## Context

There are two ways onto a ticket today, and both funnel through the private `Till::ring`
(`core/src/till.rs:825`): a scanned barcode, and a typed name with a tap on the row. The second
is the fallback for a torn label, loose goods, or a scanner that will not read, and it needs both
hands and a keyboard. Speaking is the third way: the cashier says what they have and it comes up.

Bangla, because that is the language of the shop and items already carry `name_bn`. Offline,
because a recogniser that needs the internet disappears exactly when this product is meant to
keep trading.

## What is verified

- An Apache-2.0 offline Bangla recogniser exists: `alphacep/vosk-model-small-streaming-bn`,
  shipped by sherpa-onnx as `sherpa-onnx-streaming-zipformer-bn-vosk-2026-02-09`. Streaming
  Zipformer2, ONNX, 94.1 MB fp32 (encoder 91.0, decoder 2.1, joiner 1.0). Self-reported WER:
  Common Voice 17.9, Respin 2025 16.6, Kathbath 19.3, Fleurs 20.6, Kathbath Noisy 22.5,
  IndicTTS 30.9, Banspeech 32.9.
- sherpa-onnx has a WebAssembly ASR build. Both it and the model are Apache-2.0, which is
  inbound-compatible with this repo's AGPL-3.0.

## What is not verified, and is treated as unknown

- Real accuracy on this shop's product names, in a room with a fan. Read-speech WER on Common
  Voice is not that number. Brand names and code-switched English are out of a fixed lexicon and
  are substituted with the nearest in-vocabulary word rather than corrupted randomly, so the
  errors are biased and repeatable rather than obviously wrong.
- Whether the Emscripten payload can be fetched in resumable chunks and cached, rather than
  preloaded whole into MEMFS before `main`.
- Whether any of it runs on a cheap tablet: WASM SIMD, memory, and whether a streaming model
  holds real time.

## The decision that shapes everything below

**Voice never rings.** `Heard` is read-only: it returns candidates and a *proposed* quantity, and
the screen commits with the existing `Add`. Three reasons, and any one of them is enough.

- The correction window is one ticket long. Stock moves at checkout (`till.rs:1546`), so a wrong
  line is free until then and costs money after. The only thing between the two is a cashier
  proofreading a twelve-line ticket with a queue in front of them.
- `scan` trusts a printed barcode and `add` trusts a human who looked at a row and pressed it.
  Voice would be the first path where the till decides *which item the human meant* and then
  takes money for it. Routing it through `ring` makes the rules right, not the identity.
- At a quarter of words wrong, a three-word utterance is fully correct less than half the time.

This also keeps the "no rules in the UI" line intact: nothing new puts an item on a ticket.

## Phases

### Phase 0 — fix what is already wrong (done, 2026-09-07)

Found while reading the search path for this feature. None of it needs audio, a model or a
browser, and all of it is wrong today.

- `Cart::add_item` accepted a negative quantity on an empty ticket. See `todo.md`.
- `normalise` broke Bangla words apart at the hasant and the nukta, folded neither encoding of
  ড়ঢ়য় onto the other, and kept Bengali digits apart from Latin ones.

### Phase 1 — `core::voice::understand`, text only (done, 2026-09-07)

Transcript to terms plus a *proposed* quantity plus a reason when there is none. No catalogue, no
cart. Accept a proposal only for a bare count with an explicit counter word (টা/টি/পিস/প্যাকেট)
in 1..=99 with exactly one number in the utterance. Refuse, and say why, on: a money word
(টাকা/টাকার, because "একশ টাকার চাল" is a hundred taka *of* rice and reading it as a quantity
bills a hundred kilos); a weight or volume word; more than one number; a fraction word; হালি.

Weight and volume are **not built**, and the blocker is named: `Item.unit` is free text
(`core/src/replica/mod.rs:38`), defaulted to `"Nos"` by the server and typed by hand in the back
office. Nothing in the system says whether an item is sold by count, weight or volume, nor how
many grams are in one of it. "৫০০ গ্রাম" against a 500 g packet and against loose goods differ by
a thousand times, and the till has no fact that tells them apart.

### Phase 2 — `resolve`, read-only, integer-only

Score against the existing token index. Terms that hit nothing are not dropped for free: an
unmatched term is the strongest evidence available that the utterance was not understood, so it
is counted against the match. Weight terms by rarity with integer `ilog2`, never a float, because
the same core runs on wasm32 and aarch64 and the two must not disagree about which item won.

Confidence decides only whether the screen shows one row or a list. It never rings.

### Phase 3 — `Command::Heard`, read-only, its own `View.heard`

Wired into the *existing* lookup box, so the whole feature is demonstrable and testable with a
keyboard, and a shopkeeper can use it, before a microphone exists. This is the honest place to
stop and find out whether it is worth the rest.

### Phase 4 — TLS

`getUserMedia` needs a secure context. So does `navigator.storage.getDirectory()`, which means
**the OPFS backend already has this requirement and nothing has said so**: dev runs on
`127.0.0.1` and `localhost`, which are secure contexts, and a tablet reaching the shop's server
at `http://192.168.1.x:8080` is not. The TLS terminator is listed in `todo.md` as still not
built. It is a prerequisite of this feature and, on the evidence, of the storage layer on any
real shop network.

### Phase 5 — the recogniser, offline, opt-in

Separate artifact, separate worker, int8 first and measured rather than assumed, chunked download
with resume, a storage gate (never with a shift open, never without headroom: the model shares an
origin quota with unsent sales and leased receipt numbers), and a device gate that refuses in
shop terms rather than failing at first use.

### Phase 6 — contextual biasing from the catalogue

sherpa-onnx transducers take hotwords with a per-phrase boost. Generating that list from
`replica.items()` turns an open-vocabulary problem into a nearly closed one, and is the single
change most likely to move real accuracy. Cheap, and last only because it needs the rest.

## Status log

- **2026-09-07** Phase 0 complete on branch `said-out-loud`. Two defects fixed, five tests added,
  each one confirmed to fail with its fix removed.
- **2026-09-07** Phase 1 complete. `core::voice` reads a transcript with no new dependency: a
  lexicon in `bangla.rs`, and `understand` in `mod.rs` producing terms, a proposed count and a
  refusal in words. Eighteen example tests and six properties. Every one of the seven refusal
  rules was removed in turn and a named test failed each time.
  **628 tests green across the workspace with a real Postgres attached and nothing skipped**,
  clippy clean under the strict lint set, and `core` still builds for `wasm32-unknown-unknown`.

  Two rules changed while writing the tests, both because a test said so rather than because a
  plan did:
  - A fraction or a set said with no numeral in front of it still asserts a quantity. "আধা কেজি
    চিনি" contains no number, and reading the number first answered "nothing was said about how
    many" for a sentence that plainly said something about it. Both are now read before the
    search for a number; a measure and a money word still are not, because a "কেজি" with no
    number in front of it is a word out of an item's name rather than a claim.
  - "দুটো" is two with the counter already written on. Listing it among the numerals made it a
    bare number the till then refused, which is the opposite of what the word means. It is a
    counter suffix now, like "টা" and "টি".

## Deviations

- The approved plan had a spoken barcode or SKU ring the line outright. Dropped. Internal codes
  are dense, so one misheard digit is a different real item at a different price, silently. If a
  spoken-code path returns at all it will be EAN-13 with the check digit verified, and it will
  propose rather than ring.
- The approved plan took a quantity from the front of the utterance and applied it. Narrowed to a
  proposal the cashier taps, for the reasons under Phase 1.
- The approved plan's "a number in the item's own name is not a multiplier" rule is circular: the
  number was one of the terms used to find the item, and it fails on brands that carry a number
  ("7 Up 250ml") and on two numbers in one utterance ("দুইটা চাল ৫ কেজি"). Replaced by refusing
  to guess and saying so.
