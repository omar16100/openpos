# The till screen

Rendering and input. Every decision is in the Rust core this drives, and a rule
that appears in these files is a rule the Android till does not have.

```
cd apps/till-web
cp -r ../../target/pkg public/pkg   # built by: cd bindings && wasm-pack build --target web --release --out-dir ../target/pkg
npm install
npm run dev
```

## Shape

- `src/till.worker.js` holds the till. It lives in a worker because OPFS sync
  access handles cannot be created anywhere else, which is the constraint the
  whole storage design was built around.
- `src/till.js` is one promise per command, matched by id: a barcode scanner
  fires faster than a round trip, and replies arriving out of order would render
  the wrong basket.
- `src/App.svelte` is the screen. It sends commands and renders what comes back,
  including refusals worded by the core rather than reworded here.

## What works

Opening on OPFS with the storage self-test run first, scanning, taking cash,
finishing a sale, and showing a refusal from the core. Verified in Chrome.

## What does not, yet

The catalogue arrives by sync and sync is not wired into this screen, so there
is nothing to scan against unless a catalogue is applied by other means. Ticket
ids are minted here from `crypto.randomUUID` rather than as real ULIDs. There is
no enrolment screen, so the shop and terminal are constants at the top of
`App.svelte`. All of it is in `todo.md` rather than left to be discovered.
