# The back office

Adds the people, prices and shop details a till cannot run without, and issues
the codes that enrol more devices.

```
cd apps/admin
cp -r ../../target/pkg public/pkg
npm install && npm run dev
```

Served under `/admin/` beside the till, which is why `vite.config.js` sets a
base path: absolute asset paths render a blank page there.

## Shape

It is a device like any other. It enrols with a code, keeps its credential in
its own store, and syncs, so it can show what it is about to change rather than
writing blind. The difference is the role on its code, which is what the server
checks before it lets anything here through.

It does not speak the protocol. Requests are built and replies read by the same
Rust the till uses, for the same reason: a second implementation is one that
drifts, and the one used least drifts furthest.

## What works

Shop details, people with PINs, catalogue items including the listed-price tax
rule, and enrolment codes for more tills. Verified in Chrome against a live
server, end to end: an item added here was scanned at a till minutes later.

## What does not, yet

Nothing lists or edits what is already there: every form adds or replaces, and
there is no way to see the catalogue, correct a price, or deactivate somebody.
Suppliers, goods receipts, stock counts, the repair queue and terminal health
all have routes on the server and nothing here.
