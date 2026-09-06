# todo: openpos

Started 2026-09-06. Boxes are ticked only after the work is done and evidenced.

## Design
- [x] Decide product shape: ground-up build, open core on the Postiz model (AGPL, free self-host, paid cloud)
- [x] Decide runtime: portable Node + Postgres, Cloudflare in front, not Cloudflare-native
- [x] Decide v1 scope: till, sync + tenancy, catalogue + stock + purchasing, cash + shifts + roles
- [x] Adversarial architecture review (two independent reviews, both archived)
- [x] Revise design: leased receipt numbers, no gapless promise in v1, Vite not Next for the till,
      terminal-scoped shifts, stock-count barriers, Capacitor Android shell
- [x] Write `docs/06092026_openpos_feature_spec.md`
- [x] Write `docs/c4model.md`
- [x] Re-review tech from first principles with measured evidence (`bench/`)
- [x] Revise to a Rust core with thin per-platform UIs, Flutter on Android, Axum server
- [ ] User reviews the spec

## Implementation
- [x] Cargo workspace, `openpos-core` crate, strict lints (no unsafe, no unwrap, no raw arithmetic)
- [x] `core::money`: integer Minor, Milli, Bp with checked arithmetic and half-away-from-zero rounding
- [x] `core::domain::pricing`: line totals, VAT inclusive and exclusive, discounts, ticket discount
      apportionment without drift, change due
- [x] Property tests: 7 properties, 22 tests green, clippy clean under the strict lint set
- [x] `core::ids`: ULID as u128, Crockford base32, injected clock and entropy
- [x] `core::replica`: in-memory catalogue, barcode, code and sorted token indices, delta batches,
      live on-hand. Measured 9 ns per barcode lookup, 46.8 ms to build 20k items, 4.7 MB heap
- [x] `core::cart`: sale state machine both UIs drive. Line merging, frozen prices, discount
      ceilings with supervisor override, split tenders, close into an immutable ticket
- [x] `core::lease`: server-leased receipt blocks, epoch fencing, reserve block, unnumbered sales
- [x] Storage design reviewed adversarially: five-op trait rejected, IndexedDB dropped, Web Worker
      plus OPFS, transactional commit, two stores, protocol in the core
- [x] `core::storage::frame`: envelope, CRC-32, torn-write recovery, owner check
- [x] `core::storage::backend`: six-operation contract, in-memory backend, fault-injecting backend
      that fails, tears and cuts power at any chosen operation
- [x] `core::storage::journal`: durable commit with rollback, torn-tail repair on open, A/B snapshot
      slots, checkpoint ordering, independent store lifecycles
- [x] Recovery property tests: acknowledged sales always survive, unacknowledged ones never appear,
      a completed snapshot always loads, the journal always reopens
- [x] `core::storage::wire`: postcard types mirroring the domain, schema dispatch, validation on
      decode. Cold start measured end to end: 1.9 MB on disk, 35.2 ms to a sellable indexed catalogue
- [x] `core::sync`: pull persisted before applied, cursor advanced only when durable, cold-start
      recovery from snapshot plus log replay, checkpoint policy threshold
- [x] `core::sync::outbox`: pending derived from the ledger rather than kept beside it,
      acknowledgement as a durable watermark, log emptied only when nothing is outstanding
- [x] `core::till`: the facade the FFI and both UIs call. Cold start, scan, cart, checkout with
      lease rollback on failure, sync, checkpoint, status
- [x] `core::protocol`: network types with explicit version negotiation, kept separate from the
      disk format so a wire change cannot force a disk migration
- [x] `openpos-server` ingest: idempotent by ULID, revalidates totals with the shared crate,
      quarantines rather than rejects, tenant-scoped repository trait with an in-memory impl
- [x] Server HTTP surface: Axum router, postcard bodies, push, pull with paging, lease issue,
      health, graceful shutdown. Binary runs and answers
- [x] Repository trait made asynchronous and shared-reference, so Postgres fits behind it and no
      lock sits around the whole server
- [x] Postgres repository: migrations, uuid identity, per-tenant catalogue cursor, lease blocks
      allocated in one statement, row-level security with FORCE and both USING and WITH CHECK
- [x] Verified live: 8 database tests against real Postgres as a non-superuser role, and two lease
      requests over HTTP returning non-overlapping blocks 1-500 and 501-1000
- [x] Terminal authentication: bearer tokens issued at enrolment, SHA-256 hashes stored, identity
      taken from the credential rather than the request body
- [x] Back office endpoints: repair queue, terminal health, catalogue editing. Authenticated with a
      terminal credential, because no owner role exists yet: any enrolled device in a shop can read
      that shop's queue and edit its prices. Documented, not papered over
- [x] Enrolment flow: short single-use codes with a short expiry, typo tolerant, traded for a real
      token over the wire. Revocation of one credential or of every credential a terminal holds
- [x] Rate limiting on enrolment: fixed window per client address, keyed from the connection rather
      than a spoofable header, body size capped. Verified live: ten attempts allowed, eleventh 429
- [x] Tenant export and import, needed for self-host to cloud and back. JSON Lines, keyset paged,
      idempotent, catalogue sequences preserved rather than renumbered so a till's cursor still means
      something. Not a point-in-time snapshot, and said so rather than implied otherwise
- [x] End-to-end tests: a real Till against the real HTTP server. A shop's day offline then
      syncing, a replay after a dropped reply, a cold start mid-day, and a price change that does
      not reprice an open basket
- [x] `core::shift`: terminal-scoped drawer, cash in and out with a mandatory reason, X and Z
      reports with the declared-against-expected variance
- [x] Hold and resume: the parked set written whole to standing state, restored verbatim rather than
      repriced, refused while a basket is already on screen
- [x] Refunds as the exact mirror of a sale: negated quantities, exact-balance close, the reversed
      receipt carried on the commit
- [x] Adversarial review of the whole implementation, and the fixes it produced (see below)
- [x] The `flutter_rust_bridge` gate is gone rather than passed. `openpos-ffi` is a plain C ABI of
      four functions, one of which does all the work, and Dart's FFI calls it with no code generator
      in the build. Verified from real C, linked against the built library: the same figures the
      native suite and the browser produce
- [x] The Android shared object builds end to end: `libopenpos.so`, 897 KB, an ARM64 Android ELF
      exporting exactly the four C functions and nothing else. The NDK's clang does the linking; the
      path is one developer's filesystem so it lives in `.cargo/config.toml.example`, not in git
- [ ] Run it on a device or emulator. No AVD exists here and no device is attached, so nothing has
      ever executed this library on ARM64 Android. Compiling and linking is not running

## From the adversarial review (2026-09-06)
Every fix below has a test that fails without it.
- [x] Critical: a fully drained outbox emptied the critical log and took the terminal's leased
      receipt numbers and parked baskets with it, so a shop that synced last night opened this
      morning, offline, with nothing. Standing state moved to its own A/B blob slot
- [x] Critical: recovery advanced only the active lease block, so a till that crossed a block
      boundary offline and rebooted reissued numbers already printed on receipts
- [x] High: VAT was computed before a ticket discount and never after it, overcharging the customer
      and over-declaring the tax on every ticket-discounted sale
- [x] High: the catalogue's id index was rebuilt only after a whole batch, so within a batch it lied:
      tombstones lost, items duplicated, price updates silently dropped
- [x] High: one item on two lines emitted two stock movements keyed alike, and the server discarded
      the second, so the ledger permanently undercounted what left the shop
- [x] High: the server's totals recheck read every recompute failure as agreement, so breaking the
      arithmetic bypassed the tamper check
- [x] set_qty accepted a negative quantity on a sale, which turned change due into a payout: a refund
      with no permission, no original receipt and nothing calling it a refund
- [x] Negative prices refused in the arithmetic, in the override, and at the wire boundary
- [x] `check_owner` existed to catch a cloned tablet and was called by nothing
- [x] A rollback that itself fails now poisons the journal rather than leaving a frame that the next
      commit flushes as a second sale for one basket
- [x] Bytes recovery cannot read are copied aside before truncation, instead of being destroyed at
      the one moment a person could still have recovered them
- [x] Journal sequences no longer restart at one after the log is emptied
- [x] `FaultyBackend` takes a schedule of faults, so double-fault interleavings are reachable
- [x] Duplicate lease grants ignored; unnumbered sales counted from the log so the count survives a
      reboot; `frame::encode` refuses an oversized payload instead of writing a wrong length
- [x] A catalogue pull no longer undoes stock this till has sold but not yet synced, so the count
      stops jumping back up while the cashier is looking at it
- [x] Enrolment rate limiting no longer collapses to one bucket behind a proxy. `X-Forwarded-For` is
      read only when the operator states how many proxies sit in front, counting from the right, and
      the budget is spent after the body parses so an unparseable flood cannot deny enrolment
- [x] Duplicate receipt detection had a TOCTOU window between check and store. A `receipt_claim`
      table makes the primary key decide it, inside the same transaction as the write, proved by two
      real connections racing against Postgres
- [x] `catalogue_change.payload` is versioned like every other stored payload, and a row this build
      cannot read is skipped and counted rather than failing the page, which used to turn one bad row
      into a permanent 503 for every till in the shop
- [x] Ingest is one round trip and one transaction per sale, down from three with two race windows
      between them. A per-batch transaction remains possible later; the window that mattered is shut
- [x] `SaleCommitV1.stock` is no longer trusted: the server recomputes movements from the ticket
      lines, so a self-consistent ticket can no longer decrement an item it never sold
- [x] No supervisor PIN anywhere in the core: the permission model was "the UI promises". `core::auth`
      now holds PBKDF2 credentials on the device, throttles guesses, derives the cart's ceilings from
      whoever signed in, and writes down who authorised each privileged action
- [x] Terminal tokens expire after a year and record when they were last presented, both stamped in
      the same statement that authenticates. Existing tokens keep working: expiring every live
      terminal at deploy time would take every till offline at once

- [x] Stock counts as ledger barriers: a count asserts what the shelf held at a moment and supersedes
      everything before it, on-hand is the count plus what moved after, and a sale rung before the
      count but arriving after it is excluded and raised rather than guessed at either way

- [x] Purchasing: suppliers, goods receipts with per-delivery unit cost, and the movement ledger
      generalised so a receipt is a movement like a sale. Idempotent on the receipt id, because stock
      booked twice is a shop ordering against goods it does not have
- [x] The movement ledger carries its own occurrence and arrival times, so a barrier can place a
      movement without knowing what kind of thing caused it, and on-hand needs no join

- [x] Verified the core actually compiles to wasm32, which the whole design rests on and nothing had
      ever tested. Real artifact measured: 83.6 KB gzipped against a 400 KB budget
- [x] `openpos-bindings`: the till facade as one WASM module, JSON in and JSON out, holding no rules
      of its own. Errors come back inside the view so a UI cannot silently drop one
- [x] Every public error type now carries a message a shopkeeper could act on, which the facade needs
      and which nothing else was providing

- [x] Found only by running it in a browser: `i64` parameters cross to JavaScript as BigInt, so
      `scan(code, 2000)` threw a TypeError about BigInt conversion before reaching any till code.
      The boundary now takes JavaScript numbers and refuses any that is not exactly a whole number,
      because `as i64` would have truncated 12.7 to 12 and the shop would find out at the end of day

- [x] Found in Chrome, and only findable there: an OPFS handle carries an implicit position that a
      write advances, so a read issued after a write starts where that write ended and returns
      nothing. A blob written and read back in the same breath came back empty. Left alone, a till
      would have booted on an empty ledger rather than refusing to boot at all, which is the worst
      shape this failure could take. Every read now states its offset

- [x] `core::sync::driver`: what to sync next and when to try again, decided in the core so retry
      policy cannot differ between a browser and a tablet. Sales before numbers before the catalogue,
      doubling backoff to a five minute ceiling, and no way to express giving up
- [x] Proved against the real server: twelve sales rung offline, then drained by asking the driver
      what to do rather than by a test calling the endpoints in the order it already knew

- [x] `apps/till-web`: the till screen. Svelte 5 and Vite as the spec says, the core in a worker on
      OPFS, a storage self-test run before the till opens. Verified in Chrome: a scan reaches the
      Rust core and its refusal comes back and renders, worded by the core rather than reworded
- [x] Exported `run` on the wasm binding. Every operation now goes through one entry point on both
      platforms, matching the C ABI exactly, so neither can grow an operation the other lacks

- [x] Found by the integration test and nowhere else: `more_to_pull` starts false, so a driver that
      pulled only when told more was waiting never pulled at all. A freshly enrolled till, the one
      that has nothing and needs everything, would have sat there with an empty catalogue forever

- [x] Tax base as a per-item choice: ordinarily VAT follows the discount, and for listed-price goods
      it is fixed to the price on the packet so a discount comes out of the shop's margin. 100.00 with
      10 percent off the line and 5 percent off the ticket is 85.50 plus 15.00 tax, total 100.50
- [x] The tax base reaches a till from the back office and changes what a customer pays. The note
      here previously said it did not, which was wrong: the catalogue route carries the whole item
      shape, so it worked already. Now proved rather than assumed
- [ ] Two taxes stacked on one line, such as a supplementary duty charged before VAT, is still not
      expressible: a line carries one rate. Waiting on the ordering rule rather than assuming one
- [x] The browser prints. A finished sale renders the receipt on screen and opens the print dialog,
      and the print stylesheet puts the receipt on the paper and nothing else
- [x] Shop details live on the server, are fetched by the driver before the catalogue, and are kept
      in the terminal's standing state because a receipt is printed with the internet down
- [x] `core::receipt::escpos`: the same laid-out lines as bytes a thermal printer understands. Init,
      emphasis switched only when it changes, feed, partial cut. Proved end to end through the JSON
      boundary, which is the shape the Android till will use
- [x] The till screen signs a person in and takes a refund. A wrong PIN says how many tries remain,
      an empty list of people says so rather than looking like a forgotten PIN, and a refund prints a
      receipt headed REFUND
- [x] Found by actually taking a refund on the screen: the money model assumed a sale. The exact
      button did nothing, and the screen said "Change 494.50" when nothing had been handed over.
      There is one subtraction now, and a refund is a sale with the signs turned round
- [x] The drawer is on the screen: a counted opening float, cash in and out with a reason the screen
      refuses to skip, a live figure for what the till should hold, and a close that reports the
      variance. Verified in Chrome: 2000 float plus a 494.50 sale less a 500 drop is 1994.50, counted
      at 1990 and reported as 4.50 short rather than refused
- [x] The drawer panel sits below the sale, not above it. Scanning is what a cashier does all day and
      the drawer is what they touch twice; the first field on the screen is the barcode
- [x] `apps/admin`: the back office. Shop details, people with PINs, catalogue items including the
      listed-price tax rule, and enrolment codes for more tills. Verified end to end in Chrome: an
      item added there was scanned at a till minutes later
- [x] Enrolment happens before a till is opened, because the code decides which terminal the device
      is. Opening one as a guess first is what made a second device present a credential for one
      terminal and a request body for another, and answer 403 to every lease
- [x] Each device keeps its store in a directory named for its terminal. Two apps on one origin share
      an OPFS root, and the till and the back office were opening the same files as different
      terminals. The journal's owner check caught it, which is what it is for
- [x] A worker releases the store it holds before opening another, and releases the handles when an
      open fails. Without either, every retry complains about access handles rather than the reason
- [ ] Bengali cannot be printed. No standard ESC/POS codepage carries it, so those lines are marked
      and reported rather than sent as bytes that would print as Latin mojibake. Printing Bengali
      needs rasterising it and sending an image, which needs font data
- [ ] No physical printer has been near any of this. The byte stream is right by inspection and by
      the specification; whether a given cheap printer agrees is unknown
- [ ] The receipt is not a Mushak 6.3 tax invoice and does not claim to be. Buyer BIN, the fiscal
      number from an EFD, and whatever else the form requires are absent, and the NBR rules in these
      notes are still vendor-blog sourced and unverified

- [x] Caught while checking the above: adding a field to `ItemWire` changed the stored catalogue
      payload without bumping its schema, which would have made every row written before it
      undecodable and stopped every till in every shop from pulling. Schema 2, with version 1 still
      read, and a test using the exact bytes version 1 wrote

- [x] `core::receipt`: the receipt laid out once, in the core, for every printer. Lines of text and
      an emphasis flag; turning them into ESC/POS bytes or markup is the platform's job and differs,
      laying out columns is the same job everywhere. Verified through the C ABI
- [x] Found because a receipt made it visible: every line in the facade's view reported a total of
      zero. The screen showed unit prices so nobody had noticed, and the field was a lie waiting

- [x] People. `core::auth` could check a PIN and enforce a permission since it was written, and
      nothing could create a person to check: every rule in it was unreachable. There is an operator
      table, a route an owner uses, a fetch the driver makes before the catalogue, and sign-in across
      the FFI. Proved end to end: an owner adds a cashier, a till learns them, the cashier signs in,
      and is refused the refund the owner did not grant
- [x] The PIN never crosses the network. The owner's device derives the salt, rounds and key with the
      same code the till verifies with, so the operator table is worth nothing to somebody who copies it
- [x] Found by an existing test: a shop with nobody in it yet would have asked for its people
      forever, because the driver read an empty reply as "still does not know". Settings fetches are
      now recorded as asked, like a pull, and re-asked every ten minutes

- [x] X and Z reports on the screen, both from the core. One block renders both, because a Z is an X
      plus what was counted and two blocks would show the same figures twice and let them drift. The
      variance comes from the core rather than being worked out again on the screen
- [x] A tender row says whether the money is in the till, carried on the row rather than inferred
      from its name, so a screen cannot quietly decide that a wallet counts as cash

- [x] A device whose credential the server refuses now says so and offers a way back. The platform
      reports the status, the core decides what 401 means, and the screen shows it. A failed request
      carries the view back with it, or the one fact worth showing never reaches the screen
- [x] The back office lists the shop's tills and can issue a code for one that already exists. Every
      code minted a new till id, so recovering a device meant giving it an empty ledger and stranding
      whatever the old one had not sent
- [x] A till refuses to re-enrol as a different till while it has sales to send, and says how many.
      The check is after the reply, because only the reply says which till the code is for
- [x] A cashier can correct a quantity, take a line off, discount a line, and discount a ticket. All
      four existed in the core and none could be reached from the counter, so a wrongly scanned item
      meant starting the basket again
- [x] The demo catalogue has an item taxed on its listed price. Every item was taxed the same way, so
      the demo could not show the one tax rule this product was asked for
- [x] The back office lists the catalogue and corrects an item in place. Every save minted a new id,
      so changing a price put a second copy on the shelf and nothing could show either. It answers
      from this device's own replica, so it works with the line down
- [x] `WireItem` carries the cost. Correcting a price sent a zero for it, which wiped the margin on
      every item anybody ever fixed
- [x] An item can be stopped and started. The flag was stored, honoured by search, and ignored by
      the barcode lookup, so a discontinued item went on selling to anyone holding a box of it. A
      till now refuses to ring one and still refunds one, because the shop sold it last week
- [x] Somebody can be suspended and let back in. Its own route, carrying no PIN, because the upsert
      takes the whole person including the derived key and an owner does not have it: a PIN is hashed
      where it is set and never travels. Asking for it would mean knowing a cashier's PIN to take the
      drawer away from them
- [ ] A suspension takes up to ten minutes to reach a till, which is the settings refresh. Fine for
      somebody who has left, wrong for somebody being locked out in a hurry, and the screen now says
      the ten minutes rather than implying none
- [ ] Nothing stops two people having the same name, and a till's sign-in panel then shows two
      identical buttons. Legitimate in a shop with two Rinas, and indistinguishable from adding one
      twice by accident, which is how it was found
- [x] A delivery can be booked in and a shelf counted, from the back office. Nothing but a sale moved
      stock before, so every figure in the shop walked towards zero and stayed wrong
- [x] `/v1/back-office/stock/on-hand`: what the shop believes it holds. The figure on an item record
      is whatever it was when somebody last edited that item, because a sale is not a catalogue
      change and must not bump the catalogue cursor. Showing that as stock showed a number that never
      moved, which is what the back office did for about an hour today
- [x] The demo books an opening delivery instead of asserting forty on each item record. A catalogue
      claiming stock nobody delivered is a figure the shop cannot explain, and the stock screen
      contradicted it
- [x] Suppliers can be added and a delivery filed under one. Optional on purpose: a shop that has not
      written its suppliers down should still be able to book goods in rather than being stopped at
      the door by a form
- [x] Deliveries read back, newest first, with the supplier, the challan number, the goods and what
      they cost. Proved against Postgres as well as the in-memory store, because the query is real SQL
      and the two stores agreeing is the only thing that makes the in-memory one worth testing against
- [x] `/v1/back-office/takings`: what the shop took over a period, by till, with refunds counted
      separately. Answered from the sale headers, because the total and the time are columns and
      decoding every ticket would make the question an owner asks most often the dearest to answer
- [x] A wait carries the failures behind it, so a till that cannot reach the shop says so instead of
      showing "idle". One description shared by both screens, because a till that has stopped
      reaching the shop must say the same thing wherever it is looked at
- [x] The message clearing when the shop comes back, now observed live against a Postgres-backed
      demo: held up, held up, held up, then pull. It could not be tested before because the demo only
      ran in memory and a restart refused the credential rather than resuming
- [ ] The Postgres tests need `OPENPOS_TEST_ADMIN_DATABASE_URL` and `OPENPOS_TEST_DATABASE_URL`, and
      skip silently while still reporting as passed when they are unset. Every total quoted in this
      file before 6 September counted forty eight tests that were not running: forty two in
      `postgres_repo.rs` and six in `export_import.rs`. A skip should be reported as a skip
- [x] A supplier can be corrected and retired. Every save minted a new id, so fixing a phone number
      put a second supplier of the same name in the list: the same bug the catalogue had, in the same
      place, three days apart. A retired one keeps the deliveries filed under it and stops being
      offered on a new one
- [x] A stock count is written down as it is typed, kept per shop on the device, and filed in
      batches. Paging, closing the tab or a flat battery no longer takes the afternoon with it, and a
      count interrupted at a hundred and forty shelves carries on from a hundred and forty. Line ids
      are minted once and kept, so a batch whose reply was dropped costs nothing when it is resent
- [ ] The count sheet lives in this browser's storage rather than in the device's own store. It is
      working state and can be re-walked, which is why; a device wiped mid-count still loses it
- [ ] Unexplained, seen once: a back-office device that had been running across many rebuilds showed
      its catalogue cursor past two changes it had not applied. A clean device does the same thing
      correctly, and a core test covers the incremental case. Not reproduced, not dismissed
- [x] A stranded till can be read off and carried. It lists what it is holding, including sales read
      back out of the salvage blob, and writes them out as text somebody pastes into the back office.
      Every carried sale goes into the queue a person works, because the credential that ordinarily
      says where a sale came from is exactly what such a device has lost
- [ ] What a device carries out is text somebody copies. On a shop with one working screen that is a
      message on a phone, which is fine, and on a till holding a week of sales it is a wall of hex
      with no file and no QR
- [x] One worker and one bridge in `apps/shared`, driven by a ten-line entry per app. The two copies
      had drifted twice in a day: a status reported to the core in one and dropped in the other. The
      entry is all that can differ, because the bundler rewrites the wasm path per app

- [x] `OPENPOS_DEMO=1` seeds the demo shop into whatever store is configured, so it can be run on
      Postgres and survive a restart. Memory-only made every check of anything that has to outlive a
      restart impossible against the demo, which is most of what this product claims
- [x] The demo issues two codes for two terminals, a back office and a till. One code meant both apps
      enrolled as the same terminal, opened the same OPFS directory, and failed with a complaint
      about access handles. That cost an hour today, twice
- [x] The repair queue has a screen. The takings screen counts sales needing attention per till, and
      until now there was nowhere to go and look at them, which is a pointer to a thing that does not
      exist: the same shape as every unreachable feature found this week
- [x] `cargo run -p openpos-server --example restored_till -- <server> <code>`: a till that behaves
      like one restored from a backup, so the three paths that only appear when something has gone
      wrong can be reached. A client, not a back door: an ordinary code, the ordinary push endpoint,
      nothing added to the server and nothing that can be switched on in a shop
- [x] `docs/running.md`: the commands, the settings, the tests, and the tool that reaches the failure
      paths. Every command in it was run as written before it was committed, which found two things
      wrong with what I had written from memory

- [x] `server/src/http.rs` split: the back office is its own module, handlers and tests together.
      3,384 lines down to 1,392 and 2,060, and the test count is unchanged either side of the move,
      which is the only thing that says nothing was dropped
- [x] Where a save is addressed, and what it must not quietly change, is one function in
      `apps/shared/records.js` with seven tests. Both forms that had the bug now use it. Run them
      with `node --test 'apps/shared/*.test.js'`; no runner is installed, because a dependency there
      is a dependency in the thing a shop runs
- [x] A person can be corrected: their name and what they may do, without their PIN. One route for
      that rather than one per field, because suspending and renaming are the same act from here
- [x] A PIN can be replaced. Its own route, carrying a credential and nothing else, because amending
      somebody carries none and that is the point. Derived on the owner's device with a fresh salt,
      so the digits never travel and a forgotten PIN can only be replaced, never read back
- [ ] The back office reads its lists from this device's copy of the catalogue, which is up to half a
      minute behind. Withdrawing something and correcting it inside that window used to carry the
      stale flag back and put it on sale again. Fixed for that one path by changing the row this
      device just changed; the general case, where another device made the change, is still there

- [x] A cashier can look an item up by name and ring it. A barcode that will not read, loose goods
      that carry none, a label torn off: the shop still has to sell the thing. `Replica::search` had
      one caller and it was the back office

- [x] A sale can be parked and brought back. The core has had hold, resume, held tickets and discard
      since it was written and nothing called any of them, so a customer who went back for something
      held up everybody behind them. That is the eighth finished core feature found this week with no
      caller

- [x] Audited the core for capabilities nothing reaches: 195 public functions, and of the ones a
      person should be able to use, nine had no caller anywhere outside the core. Three are now
      wired: giving up on a basket, taking back money entered by mistake, and the checkpoint
- [x] Something calls `checkpoint_if_needed`. Nothing did, so the catalogue delta log grew for the
      life of a device and every boot replayed all of it: a till taking longer to open every morning
      for a reason nobody in the shop could see. Asked after each sale, between customers, and the
      till decides whether the log is long enough to be worth folding
- [x] A price can be overridden by somebody who may. The permission was stored, checked and
      unreachable: `Cart::set_unit_price` enforced it and `Till` never forwarded it
- [x] A sale can be paid by wallet, card or on account, and split across them. The core has known
      about all three since it was written and the drawer report already split by them; the till took
      cash only, in a country where a shop takes bKash and Nagad all day
- [ ] Still unreached, from that audit: `set_customer` and `restore_line`
- [x] Which wallets a shop takes is set once in the back office and offered by name at the till. The
      standing state went to schema 2 to carry them, with the version 1 shape kept for reading what
      the build before wrote

- [x] Every decoder reads the schema its bytes carry, not this build's constant. The standing state
      was wrong and the snapshot was wrong in a second way: the frame said schema 1 while the payload
      had been version 2 since the tax base was added to an item. Nothing noticed because nothing
      read the label
- [x] A snapshot this build cannot read is treated as absent and reported, not fatal. It is a cache:
      refusing to open would be a till that will not sell because its copy of the prices is stale

- [x] `core/tests/schema_labels.rs`: a day's work through the real writers, then every frame read
      back and its label checked against the schema its payload uses. Proved by reintroducing the
      snapshot bug and watching it fail. Both blobs and all four log kinds

- [x] A sale on account names who owes it, on the paper and in the ticket. Offering "on account"
      without that, which is what shipped an hour ago, is a way to record money the shop has given
      away and cannot chase
- [x] There is an account book. A sale on account becomes a debt against the name the cashier typed,
      folded so spacing and case do not split one person in three, and the back office lists who owes
      what, takes payments against it and shows what a balance is made of. Balances are summed from
      entries and never stored: a stored balance and a ledger that disagree is a question nobody in a
      shop can answer
- [x] A shop can write down who buys on account, with a phone number, and a till is told the list so
      a sale can be written to somebody's account with the line down. A sale naming one of them lands
      on that person whatever the cashier typed, and the typed spelling is still what is shown back,
      because it is what is on the receipt in their hand
- [x] An export carries who buys on account, so a shop that moves machine does not arrive with a
      book of ids nobody can put a face to
- [x] One question at closing, answered in one call: what was sold, what came back, what the drawers
      held against what they should have, and what went on account rather than into the till. The
      takings route it replaces is gone rather than left beside it, because two routes answering one
      question is the thing that drifts
- [x] A till can answer "how much do I owe" across the counter. Balances are asked for on their own,
      more often than the names, and never shown without saying how old the figure is; they are not
      written to the device, because a number carried through a night is worse than none when
      another till may have sold to that person since
- [x] What the shop owes the revenue, by rate, for a month. Worked out when each sale arrives and
      stored beside it, so the question a shop asks twelve times a year is one query rather than a
      month of tickets decoded. Recomputed by the server rather than read from the payload, and on
      import too: what a shop declares must not be something a file could assert
- [x] The other half of the book: what the shop owes its suppliers, being the deliveries less what
      has been paid, with payments recorded against a supplier and idempotent on a minted id. A
      delivery paid at the door is a delivery and a payment on the same day, which is what the paper
      says too
- [ ] A supplier's balance is every delivery ever booked against them less every payment. A shop
      that has been running for two years and settles weekly will want a period, and the screen has
      no way to ask for one
- [ ] The VAT summary counts what was sold, not what was collected, and says nothing about a sale
      still sitting in the repair queue. A quarantined sale is in the figures like any other, which
      is right for goods that left the shop and wrong if the queue entry turns out to be a duplicate
- [ ] A sale on account against a name nobody wrote down is still keyed on the folded name, so two
      unregistered Karims still share an account. That is what the paper notebook does and what a
      shop that has written nobody down gets; writing them down is the answer and is now possible
- [x] An export carries the account book, entry for entry, because a payment is in no sale payload
      and a shop that arrives with its sales and none of what anybody owes it has lost the part it
      cannot rebuild. A debt can also be struck off with a reason, so a sale rung twice by a restored
      till can be corrected without recording a payment nobody made
- [x] An export carries the counted drawers too, with who counted each. A shop that moved machine
      and arrived unable to say a single evening was ever reconciled had lost the accountability
      record that is the entire reason one person counts a drawer and another reads it
- [ ] A quarantined sale on account still goes on the book. It has to: goods left the shop and the
      queue is note-only, so refusing to record it would lose a real debt with no way to add it
      later. The correction is to strike it off with a reason, which is now possible and is a person
      noticing rather than the machine deciding
- [ ] Who owes and what an account is made of are read whole, with no cursor. Five hundred accounts
      and two hundred entries are the ceilings, and past them a screen quietly shows less than the
      truth

- [x] An item can have a Bangla name, and a cashier can find it by typing one. The catalogue has
      carried the field and the search has indexed it since both were written, and nothing could set
      it: every item's Bangla name was a copy of its English one
- [ ] The receipt still prints the English name. A screen renders Bangla and thermal paper does not,
      which is a raster path and font data this crate has no business carrying. The unprintable lines
      are already reported per line, so a platform that grows one knows exactly which to draw

- [x] A shop can say that its shelf prices already include the tax, and what it sells a thing by.
      Both were hardcoded on the way out: `price_inclusive` was always false and `unit` was always
      "Nos". The first one overcharged every customer of a shop that prices inclusive, which is most
      of them
- [x] The receipt prints the unit. It was not a layout change as I said when I wrote that line: a
      receipt prints from the ticket, and the ticket's lines had no unit, so it needed the cart line,
      the stored line and a sale schema bump with the version 1 shape kept
- [x] Swept the wire shapes for fields that only ever receive a literal, which is what `name_bn`,
      `price_inclusive` and `unit` all were. One hit left and it is legitimate: the demo seeding path
      applies a catalogue at cursor zero on purpose

- [x] `core/tests/upgrade.rs`: one device whose standing state, snapshot and unsent sale were each
      written by a build that predates a different field, opened on the morning after. Each legacy
      path had its own test and none described what a real device holds
- [x] A device says when it could not read its stored catalogue and is fetching it again. I set that
      flag yesterday, wrote "reported" in the commit message, and nothing read it: the twelfth
      unreachable thing this week and the first one I made myself
- [ ] An unreadable sale stops a till opening, where an unreadable snapshot does not. That asymmetry
      is right, because a sale is the only copy of money that changed hands and a snapshot is a
      cache. It also means forgetting a legacy path on the sale format takes every till in every shop
      out at once, which is what the upgrade test is now standing guard over

- [x] A counted drawer reaches the shop. Shift data never left the till: a cashier counted, the till
      worked out the variance, and the owner had to take their word for both. The count is held in
      the standing state so it survives the critical log being emptied, pushed ahead of the
      catalogue, and read back in the back office
- [x] Who counted the drawer is written down at the time and read back in the back office. The name
      is copied beside the id rather than joined later, because somebody who has since left the shop,
      or been renamed, is still the person that variance belongs to. A drawer counted by an older
      build keeps its count and carries no name, which is the truth about it and better than a guess
- [ ] Who counted a drawer is what the till said, not what the server checked. A device holding a
      credential can report any name against a count, the same way it can report any total. Worth
      revisiting when a taken device is a scenario with a drill: unenrolling is the answer today
- [x] A till says what an open drawer holds while it is still open, every couple of minutes, and the
      back office lists what is open now with how stale each figure is. A drawer left open overnight
      and wiped in the morning now costs the last two minutes of it rather than the whole evening,
      and an owner at closing time can see which tills nobody has counted

## Open, and named rather than left implied
- [x] Token renewal: `/v1/renew` trades a working credential for a fresh one, authenticated with the
      one being replaced. The old one lapses after a day rather than being revoked, because the reply
      can be lost and a till whose only credential vanished mid-request is a shop offline until
      somebody re-enrols the tablet by hand. Renewing never extends a deadline
- [ ] Per-batch ingest transaction. One per sale now, down from three; the race window is shut, the
      latency of a full-day drain over mobile data is not yet measured
- [ ] An export is not a point-in-time snapshot: each page is its own transaction, so a till syncing
      mid-export can be missed. Idempotent import is what makes it safe, and re-running converges
- [x] OPFS backend written and proved in Chrome: a sale rung in a worker survives that worker being
      destroyed, and a brand new till reads it back from disk. Cold-start-offline, demonstrated
- [x] A storage self-test the platform can run at boot, kept rather than deleted once it worked: the
      difference between "this browser will not flush" and "this ledger is corrupt" decides whether
      somebody restores a backup or buys a different tablet
- [x] The module has been loaded and run by a real browser. `wasm-pack` installed, glue generated,
      page served, sale rung in Chrome: net 860.00, VAT 129.00, total 989.00, change 11.00, matching
      the native suite exactly. 74.3 KB gzipped whole, 5.1 ms to instantiate, 0.300 ms per scan
- [x] Manual stock corrections: breakage, spoilage, theft, a sample given away, a count that was
      wrong. Movement kind 3, owner only, a reason required by the schema and refused when blank,
      idempotent on the correction id, and superseded by a later count like any other movement
- [x] Back office routes now need an owner credential. A till may ring sales and sync and nothing
      else, so a tablet left on a counter is no longer the whole shop. Existing credentials became
      owners in the migration, because that is what they demonstrably were

- [x] Enrolling a second device is a workflow: an owner asks for a code naming a new terminal and a
      role, the server creates the terminal before the code exists, and the new tablet redeems it for
      a credential of that role. A caller may not grant a role above its own, and a code cannot be
      left standing longer than an hour

## Next
- [ ] Implementation plan document, once more of the core shape is proven in code
- [ ] Decide whether the Android UI is Flutter at all. The C ABI removes the reason to prefer it
- [ ] Resolve open questions: NBR primary source, printer models to certify, Android distribution,
      DCO before first external PR, hosting substrate for the paid tier, browser storage backend
