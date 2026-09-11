# Plan: a cashier says what they want, and it comes up

Purpose: add a third way onto a ticket, for a shop where one hand is holding the goods.
Status: phases 0 to 3 done and evidenced, and working in a browser. Phase 4 (TLS) next,
and it blocks everything after it.
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

### Phase 2 — `resolve`, read-only, integer-only (done, 2026-09-07)

`Replica::weigh` scores against the existing token index instead of intersecting it, because
typing is narrowing and speech is not: a cashier means every word they type, and a recogniser
adds words nobody said and drops words they did. An unmatched word is not dropped for free, it
is charged at the maximum weight, because a word the catalogue has never heard of is the
strongest evidence available that the utterance was misheard.

Rarity by integer `ilog2`, never a float: the same core runs on wasm32 and aarch64 and two builds
that rounded a score differently would disagree about which item the cashier meant.

Confidence decides only whether the screen shows one row or a list. It never rings.

### Phase 3 — `Command::Heard`, read-only, its own `View.heard` (done, 2026-09-07)

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

#### Which model, and why

Checked against the published catalogues on 2026-09-07 rather than from memory. For Bengali the
field is small: **the sherpa-onnx zoo holds exactly one Bengali transducer.**

| Model | Type | Size (fp32) | Licence | In the zoo |
|---|---|---|---|---|
| **`vosk-model-small-streaming-bn`** | Streaming Zipformer2 transducer | **94.4 MB** (enc 91.0, dec 2.1, join 1.0) | Apache-2.0 | **Yes** |
| Dolphin base / small | CTC, **not a transducer** | 80.7 / 191.5 MB int8 | — | Yes |

**Chosen: the vosk Zipformer2**, for three reasons in this order.

1. It is a zoo entry the wasm build consumes directly, with no export or conversion step.
2. 94 MB, falling to roughly 25-30 MB at int8. On Bangladeshi mobile data that is what makes an
   opt-in download arguable at all.
3. Transducers take hotword biasing in sherpa-onnx, which is Phase 6 and the largest lever
   available, because a shop catalogue is a closed vocabulary. The Dolphin CTC models forfeit it,
   which is why a smaller CTC model is not the bargain it looks.

Deliberately not a Whisper derivative, though Bengali fine-tunes exist and score better on clean
read speech. Whisper's decoder is autoregressive and language-model shaped, and its characteristic
failure on unclear audio is confident, grammatical invention. At a counter a fabricated product
name is worse than a garbled one, because a garbled one shows up as a bad match and an invented
one shows up as a good one.

#### Two things this decision is weak on, said rather than buried

**Push-to-talk means streaming buys nothing.** Streaming pays accuracy for latency by only ever
seeing the audio so far, and the till has the whole utterance before it asks anything. There is no
non-streaming Bengali Zipformer to swap to, so this is a known tax with nothing to spend it on:
worth revisiting if a non-streaming Bengali transducer ever appears in the zoo.

**WER is the wrong metric.** Every number quoted here is read speech on Common Voice, Fleurs and
Kathbath. What decides this feature is whether the right item comes up out of twenty or forty
product nouns, through confidence rules that already refuse on thin evidence. A model with worse
headline WER but better on "মিনিকেট", "সয়াবিন" and "রূপচাঁদা" wins outright.

#### The gate

Before any of Phase 5 is built: record the demo catalogue's names spoken aloud, run the model
natively (no browser needed), and score **hit rate on product names**, not WER.

The gate is absolute rather than a comparison, because there is nothing left to compare against:
this is the only Bengali transducer in the zoo. So the question is not "is it the best available"
but "is it good enough to be worth the download", and if it is not, the answer is that the
recogniser is not built and the typed box stays.

Weight the test towards Bangladeshi speech. The card's worst figure is its Bangladeshi one,
Banspeech at 32.9 percent against 17.9 on Common Voice, and that gap is the most honest number on
it and the closest thing to a shop.

#### Supply risk

Bengali is **absent from the official Vosk model list**. This model exists only as a Hugging Face
repo and a sherpa-onnx release asset, which is thinner provenance than the rest of the zoo.
Whatever is picked gets vendored with a pinned checksum rather than fetched by name.

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

- **2026-09-07** Phase 2 complete. `Replica::weigh` and `voice::resolve`, both read-only, both
  integer-only. Twelve more example tests and two more properties, including the safety one: an
  utterance corrupted the way a shop corrupts one (a word lost to a fan, a word gained that
  nobody said) may find nothing, or offer a list, or be less sure, but may never become sure of a
  *different* item than the clean sentence pointed at. That is the failure a cashier cannot see,
  because the screen looks exactly as confident as it does when it is right.

  **642 tests green across the workspace against a real Postgres, nothing skipped**, clippy clean,
  `core` still builds for wasm32.

  Three deviations, each forced by a test rather than chosen:
  - A confidence rule was **deleted**. "Two matching words, unless the one word belongs to this
    item alone" could not be exercised: if the best candidate matched only one word, every other
    item carrying that word scored identically and the runner-up rule had already refused. Rather
    than leave a rule whose presence and absence no test could tell apart, it is gone and the
    reason is written where it stood. Breaking each of the three that remain fails a named test.
  - How common a word is now counts only goods the shop still sells. The index carries withdrawn
    items so a refund can find them, and letting those count made the one item still on the shelf
    look like one of a crowd, so the till stopped being sure of the only answer there was.
  - A word said twice is one piece of evidence. Counted every time it appeared, a stutter
    outweighed the unheard-of words that should have raised the doubt, and the till talked itself
    into being sure of an utterance that was mostly noise.

- **2026-09-07** Phase 3 complete, and the feature is usable. `Command::Heard` is read-only and
  carries its own `View.heard`, so both platforms get it and neither can grow it differently. The
  till screen takes a whole phrase in the lookup box that already existed, shows what it made of
  it, marks a match it is not sure of, and puts the offered quantity on the button so what is
  about to be rung is what the cashier is looking at when they press it.

  **Verified in Chrome against the real server and the demo catalogue**, not only in tests:
  - "ভাই একটু চাল দাও" gives Rice Miniket 5kg, with "চাল" kept and "ভাই একটু দাও" shown as set
    aside. The typed search finds nothing for that sentence.
  - "মিনিকেট চাল ৫ কেজি" offers the rice and **no quantity**, with the core's own sentence about
    weights on screen. Five bags at 2,150 for a customer buying one at 430 is the thing this
    exists to stop, and it is stopped in the real app.
  - "একশ টাকার চাল" offers the rice, refuses the hundred as money, and marks the row "not
    certain, check before pressing".
  - "তিন প্যাকেট চাল" offers "3 × Rice Miniket 5kg", and pressing it rings three at 430 plus VAT:
    1,483.50. That press is `Add`, the same one the lookup list has always used.

  Nothing said to the till reached a ticket in any of it, which is a test at the boundary as well
  as an observation: making `Heard` ring its confident candidate fails two named tests.

## What is left

Phase 4 is TLS, and it is a hard gate: `getUserMedia` needs a secure context and so does
`navigator.storage.getDirectory()`. Nothing with a microphone can be built until it exists, and on
the evidence the storage layer already needs it on any real shop network.

- **2026-09-11** Merged into the mainline, and the first number nobody had. `core/examples/voice_accuracy.rs`
  asks the question underneath the gate: whatever a recogniser gets wrong, does the reading survive
  it? If the answer were no, no model would be worth ninety-four megabytes and the feature should
  not be built.

  A shop of 52 items with 12 asked for, utterances corrupted the way a transducer corrupts them
  rather than with random noise: words dropped, words substituted **from the shop's own vocabulary**
  so the wrong word is one that matches something, conjuncts mis-segmented, digits swapped.

  | words wrong | right item offered | right item first | confidently wrong | worst single item |
  |---|---|---|---|---|
  | 0% | 100.0% | 100.0% | 0.00% | 100.0% |
  | 10% | 99.9% | 98.1% | 0.00% | 93.0% |
  | 20% | 99.5% | 95.5% | 0.15% | 86.2% |
  | 30% | 99.1% | 92.2% | 0.19% | 79.8% |
  | 40% | 98.1% | 89.7% | 0.48% | 76.5% |

  The third column is the one that decides whether this can go in a shop, because it is the only
  outcome a cashier cannot tell from a correct one. It stays under half a percent at two fifths of
  words wrong, which is worse than any figure published for this model. The reading absorbs
  recogniser error, so the model is worth measuring for real.

  **What this is not.** The corruption model is this project's own invention, applied at random. A
  real transducer makes the same substitution every time it meets the same sound, so a particular
  product could be reliably unfindable where an average over random draws calls it occasionally so.
  The worst-item column exists to catch the shape of that and does not stand in for a recording.

  AI4Bharat's models are out of scope by instruction, so the comparison they were in is gone and the
  gate is absolute rather than relative.
