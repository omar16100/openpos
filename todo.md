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
- [x] Exempt and zero rated are kept apart, from the item an owner classifies through the line, the
      paper and the shop's own recomputation to the return. A rate of zero was all this modelled, and
      the two are declared in different places, one carrying a credit for the tax the shop paid on
      its own inputs and the other not. Which goods are which is still the revenue's word and the
      shop's to set; nothing here decides it. A line that is not standard is taxed at nothing
      whatever rate the item carries, so an exempt item somebody left at fifteen percent charges
      nothing rather than taking money from a customer and declaring it to nobody. Walked live
      against Postgres: 8000 zero rated, 10000 exempt and 43000 at fifteen percent, declared as three
      rows
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
      the specification; whether a given cheap printer agrees is unknown. Nothing reaches it either:
      `Command::Escpos` and `View.job` are sent and read by no screen, so the renderer is a library
      with tests. Wiring it means choosing how a browser talks to a printer, which is a decision to
      make with the printer in hand rather than without one
- [x] Found by sweeping again: the back office never folded its own log. It pulls the catalogue like
      any device, so the log grew for the life of the device and every boot replayed all of it. The
      till folds between customers; this asks after every sync round and the core decides whether the
      log is long enough to bother. The same defect the till had before anything called it
- [x] Also from that sweep: `Till::authorise_override` was public and called by nothing. It writes a
      waiver onto a ticket, which is what a supervisor's PIN buys, so a platform could have waived
      anything by calling it directly. Removed; the one path is through `authorise`
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
- [x] A suspension reaches a till in about half a minute rather than ten. The people, the shop and
      the account customers move together from a till's point of view, so one counter covers all
      three: a till asks for that number on the cadence it pulls the catalogue at, and asks for the
      lists themselves only when it has moved. Trading does not move it, which is the property that
      makes asking often affordable
- [x] `docker compose up` gives what the architecture notes said it gave. It gave Postgres and
      nothing else: no image, no Dockerfile, and the api, caddy and backup sidecar in that sentence
      were a plan written in the present tense. There is an image now, built in three stages, and it
      carries both apps and serves them itself, which is the other thing two comments claimed and
      nothing did. Checked by building it, bringing it up, and driving a real client at it
- [ ] Still not built, and now named rather than implied: the TLS terminator, the sidecar that takes
      the nightly backup, and the billing a hosted tier would compile out. The backup today is a
      person running `openpos-server export`
- [x] A till can be opened on a directory of files, so the C ABI has a store that survives a reboot.
      Android could only open the one whose type name says nothing survives a reload, which made the
      whole boundary a demonstration. One file per blob and per log, blobs replaced by rename so a
      torn half cannot lose a terminal's numbers, and a boot report handed back through the boundary
      so a platform can say what it found. Proved by ringing a sale, dropping the till, opening the
      directory again, and by tearing the log and watching it open on what is whole
- [ ] What `flush` promises on macOS is weaker than on Linux: `fsync` there asks the drive to write
      its cache and does not wait, and the call that does needs unsafe, which the workspace forbids
      outside the C ABI. Said in the module rather than assumed. Android and Linux get a real barrier
- [x] The core's own guards were broken one at a time too, and every one was caught: a PIN that
      verifies anything, a receipt number handed out twice, a credential that answers for any shop,
      a frame whose checksum is ignored, and a failed commit that keeps the number it took. The
      method is written down in docs/running.md, because a test that passes when the code is broken
      is not a test and the only way to know is to break it
- [x] The server refuses to run as a role that can see every shop. Taking all 26 explicit tenant
      predicates out of the Postgres queries changes no answer, which proves row level security is
      carrying the boundary on its own, and equally that a role bypassing the policies has no
      boundary at all. One word in a connection string does it and it looks exactly like a working
      server, so it is asked at startup and refused, for serving and for exporting alike
- [x] The guards were checked by breaking them one at a time and watching a test fail. Eight of nine
      were caught; the ninth, the one keeping a struck-out sale from taking a delivery or a
      correction with it, was covered by nothing and now has a test that fails when it goes. A guard
      nothing tests is one the next person tidies away
- [x] The money that crosses a counter has properties, not only examples. Two of the defects found
      this week passed every example test there was, because every example paid the exact amount:
      change never exceeds the cash handed over, a closed ticket balances, and a drawer expects the
      float plus the cash that stayed. Each was checked by reverting its fix and watching the
      property fail, which is the only way to know a regression test is one
- [x] Cash cannot cross a drawer with nothing said about why. The type has said "refuses to let one
      be recorded without an explanation attached" since it was written and took whatever it was
      handed; the till screen was the only thing enforcing it, and its own comment said the core did
      too. Money out with nothing beside it is indistinguishable from theft when the count comes up
      short, and the person who answers for the drawer is not the person who took it
- [x] A stock correction with no reason is refused by both stores. Postgres refused it and the
      memory one did not, which is a store tests pass against and production does not
- [x] A barcode belongs to one item. The replica has said "the back office is responsible for not
      issuing one" since the day it was written, and nothing was: two items could carry the same
      code, and a till rings whichever its index happened to keep, at the wrong price and the wrong
      tax rate, with nothing on any screen to say why. Saving is refused and the refusal names the
      code. A withdrawn item gives its barcode back
- [x] A refusal reaches a screen in the shop's own words. Every platform threw the body away and
      showed the status number, so "409" had to be guessed at: the core puts a refusal into words
      and the worker hands those on, which is how a screen can say which barcode is taken without
      deciding for itself what the server meant
- [x] Two customers with the same name are told apart too, and the shop is warned before it writes
      a second one down. Two records for one person is two accounts: what they took goes on one and
      what they paid on the other, and neither balance is theirs. The same three tested functions
      serve the people who sign in and the people who buy on account, because the records are the
      same shape and the mistake is the same mistake
- [x] Two people with the same name no longer make two identical buttons. A cashier pressing the
      wrong one hands that whole shift to somebody else: every sale, every drawer opening and every
      waiver attributed to a person who was not standing there. Where two active people share a
      name, and only there, both screens show the tail of the id beside it, so an owner can say "you
      are the Karim ending 7QF3". The back office says so once when a name is taken and then allows
      it: a shop can have two Rinas, and the answer is a name that tells them apart rather than a
      form that will not save. Worked out in `apps/shared/people.js` with tests, because a mark the
      two screens disagreed about would be worse than no mark
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
- [x] The Postgres tests say when they did not run. They need `OPENPOS_TEST_ADMIN_DATABASE_URL` and
      `OPENPOS_TEST_DATABASE_URL`, returned early without them, and a test that returns early
      passes: every total quoted in this file before 6 September counted forty eight tests that were
      not running. One test in each file now runs without a database and fails without one, saying
      what to start and what to set. A skip reported as a pass is a failure nobody will look for
- [x] A supplier can be corrected and retired. Every save minted a new id, so fixing a phone number
      put a second supplier of the same name in the list: the same bug the catalogue had, in the same
      place, three days apart. A retired one keeps the deliveries filed under it and stops being
      offered on a new one
- [x] A stock count is written down as it is typed, kept per shop on the device, and filed in
      batches. Paging, closing the tab or a flat battery no longer takes the afternoon with it, and a
      count interrupted at a hundred and forty shelves carries on from a hundred and forty. Line ids
      are minted once and kept, so a batch whose reply was dropped costs nothing when it is resent
- [ ] The count sheet lives in this browser's storage rather than in the device's own store. It is
      working state and can be re-walked, which is why; a device wiped mid-count still loses it.
      Both stores are now covered by the same persistence grant, which was the part that mattered:
      before, nothing had ever asked the browser to keep any of it
- [x] The apps ask the browser to keep what they hold, and say so when it refuses. Nothing had ever
      called `navigator.storage.persist()`, so every device's store was evictable: unsent sales, the
      receipt numbers a terminal had been given and the parked baskets, all of it discardable under
      storage pressure and discarded outright by Safari after seven days of the tab not being
      opened. The back office is the likeliest victim, being opened once a week, and it did not even
      show what its storage was
- [x] A count sent twice keeps what arrived first in both stores. The memory one overwrote and
      Postgres ignored, so a resend carrying different numbers moved a barrier in one and not the
      other: a shelf figure that depended on which store a shop was running
- [x] Explained, and it was not the till. A catalogue change the server cannot decode is passed over
      on the way out and the cursor still moves, which is right: failing the page would stop every
      till in the shop syncing for ever over one bad row. The count went into a field the pull
      handler ignored, so nobody could ever be told. The server now says it in the log as it
      happens, and an owner can ask which changes never reached the tills and set those prices
      again. That is what a device running across many rebuilds had seen
- [x] A stranded till can be read off and carried. It lists what it is holding, including sales read
      back out of the salvage blob, and writes them out as text somebody pastes into the back office.
      Every carried sale goes into the queue a person works, because the credential that ordinarily
      says where a sale came from is exactly what such a device has lost
- [x] What a device carries out can be saved to a file, copied, and checked. Text on a screen is
      fine for a shop with one working device and a message on a phone; on a till holding a week of
      sales it was four thousand characters nobody selects on a tablet. It writes a file named for
      the terminal and the day, it copies to the clipboard, and both ends show the same mark, so a
      paste that got cut short is caught rather than taken in as fewer sales than the device holds.
      The back office opens the file, and a paste a messaging app wrapped is accepted: those line
      breaks are not the shop's doing. Still no QR, which a bundle this size will not carry anyway
- [x] Nothing a supervisor allowed ever worked in a browser. The screen sends the action back as the
      core named it, and what the core named comes out of the view, which in Svelte 5 is a reactive
      proxy: posting one to the worker throws "could not be cloned", so the command never ran and
      the screen showed a message about postMessage. Every discount over a ceiling, every price
      typed over the catalogue's, every refund and every drawer opened outside a sale, in both apps.
      Proved by watching it happen and then watching it work. The bridge now posts a plain copy,
      because a screen that forgets is a screen that works until somebody tries the one path that
      reads from the view; strings pass through untouched, which is what the large payloads are
- [x] The button that gives a device that lost its credential a new code could not be pressed. Both
      buttons in a till's row were placed in the same grid cell, so the one that stops a till dead
      was drawn over the one that fixes it. Found by looking at the screen, which is the only way it
      could have been found
- [x] The back office loaded everything twice in two places, and the shop's own settings were in one
      of them: a device that had just enrolled showed an empty form over a shop with a name, an
      address and a rule about the shelf. One list of loaders now, called from both
- [x] A sale rung in a browser, end to end, against the real server and Postgres: a shelf counted to
      nothing in the back office, the till told about it through the shop fetch and the stock fetch,
      the scan refused in the core's own words, a supervisor's PIN allowing it, the refused scan
      retried without anybody retyping it, the line rung with the shelf note under it, the sale
      committed, printed, synced and read back in the back office as 494.50 for one sale. Which is
      the first time the whole loop has been walked in a browser rather than asserted in Rust
- [x] A quantity can be typed at the till, which is what a shop selling loose rice does all day. The
      line editor moved by one and nothing else, so a kilo and a half was two presses of nothing and
      the core's thousandths were unreachable from the screen that needs them. The same parser the
      back office counts shelves with, moved into `apps/shared/quantity.js` so 1.5 means one thing in
      both places and 1.5005 is refused in both. Checked in a browser: 1.5 of a 185.00 line reads
      net 277.50, VAT 41.63, total 319.13, and 1.5005 is refused with the line left as it was
- [x] An amount off, which is what a shop here says: twenty taka off, not four point six five percent
      off. A v1 line the core had always carried and no wire command or screen offered, and the
      reason it could not simply be offered: `check_ceiling` looked at rates and waved amounts
      through, on the grounds that an amount is "bounded by the line itself", which is a bound of a
      hundred percent rather than the one the shop set. A cashier with no ceiling at all could have
      given a line away by naming its price. Now measured as a share of what it comes off, rounded
      up because this decides whether somebody may give money away, and refused by the same route
      with the same words, so a supervisor allows it the same way
- [x] The till told a cashier their own twenty taka was the basket's. The screen decided between "off
      this line" and "this line's share of the ticket discount" by whether a rate was set, so an
      amount off one line read as somebody else's discount. The view carries the line's own amount
      now, and the note says which it is, and both when both apply
- [x] A test named a fixed path in the temp directory, so two test processes at once had one delete
      the file the other was about to open, leaving a store where a file was expected. It failed
      days later for a reason nobody could reproduce, which is what a shared name in a shared
      directory buys. Named for the run now, like the store beside it
- [x] A barcode nobody's catalogue has can be written down at the counter and sold, which is the
      cold-start promise the spec calls 7.4 and nothing implemented: a delivery arrives during an
      outage and a till that can only say "no such item" loses the sale, so the shop sells it off the
      paper and reconciles nothing. The cashier says what it is and what it costs; the till holds it
      like any other item, sells it, and sends it to the shop ahead of the catalogue pull because the
      sales already sent name it. Kept in the standing state beside the counted drawers, for the same
      reason: the log that holds the sale is emptied when the sale is acknowledged. Terminal state at
      schema 9, with 8 still read
- [x] The shop marks what a till wrote down and lists it for somebody to look at. A price typed to
      get a queue moving is not a price the owner agreed to. The list is read out of the catalogue as
      it stands rather than from a second list, and the mark comes off by the ordinary item save,
      because there is no second way to agree to an item. A barcode the shop has since given to
      something else is kept by the shop's item and dropped from the till's, so nothing scans two
      ways and the pair is visible to whoever works through the list
- [x] Somebody who buys on account can be written down at the till, which is three v1 lines the spec
      asked for and none of them existed: a customer created at the till offline, a phone number to
      tell one from another, and the buyer's BIN. It also closes the hole underneath them, that a
      sale on account against a name nobody wrote down was keyed on the folded name, so the second
      Karim paid for the first one's rice. Held in the standing state and sent like the items, kept
      on the screen when the shop's own list arrives without them, and the shop holds them by id
- [x] The buyer's BIN is on the paper. A tax invoice here names the supplier's and the buyer's; the
      shop's has been at the top of every receipt and the buyer's had nowhere to live. Carried on the
      customer, printed only when there is one, kept through a backup and a restore. Which found the
      bug in the middle of it: the till dropped the BIN the shop sent every time it re-read the list
- [x] The account path walked in a browser: a sale on account for somebody in nobody's list, written
      down at the till with their phone, the basket pointed at them, the receipt naming them, the
      shop holding them by id with the phone, the owner adding their BIN in the back office, and the
      next receipt for them carrying both BINs, the shop's and theirs, which is what a tax invoice
      here has to name
- [x] The sync loop runs in the worker now, not on the screen's thread. A browser throttles a hidden
      page's timers to about once a minute and can stop them altogether, so a till whose tab was not
      in front had quietly stopped sending: seen twice, both times cured by reloading. The bridge
      carries a message nobody asked for to make it possible, and the two channels are tested against
      a fake worker, including a round landing while a scan is in flight. Verified live: somebody
      written down at a till whose tab was behind the back office reached the shop without anybody
      touching that tab
- [ ] What that does not fix, and is worth saying: a tab the browser freezes outright takes its
      workers with it. Chrome freezes background tabs after minutes in some conditions, and nothing
      here notices. A till is the front tab all day, so this is a second-order worry, but the honest
      statement is that the loop is now throttled less rather than immune
- [x] A refund is now answered for. The receipt it reverses has always been in the sale's own bytes
      and nothing read it: a refund against a receipt this shop does not have, and the same receipt
      refunded twice, both went straight into the takings with nothing said. The oldest trick at a
      counter, and a shop had no way to see it. Both are held for a person now, in the same words as
      every other held sale, with what the receipt was rung for and what has been given back against
      it. Neither is refused: the goods came back and the money went out, and refusing would leave
      the only record of that on a tablet. A refund somebody struck out gives nothing back
- [x] And the goods, not only the money. A refund of the same taka made of something else, or of
      twice as much at half the price, puts stock on the shelf that never left it, which is how a
      count is made to agree with a shelf somebody emptied. Netted out of the movements the shop
      already keeps rather than by decoding sales again: a sale's movement is negative, a refund's is
      positive, and anything above zero came back more than it went out. Held for a person like the
      rest, and a struck-out refund brought nothing back
- [x] A sale nobody paid for is held. The server recomputed what a ticket came to and never asked
      whether anybody handed it over, so a payload with its tenders taken out recomputed perfectly
      and went into the day's takings: money the shop would look for and never find, on a sale
      nobody owes. The invariant is what a drawer holds rather than what the tenders come to: handed
      over, less handed back, is the total. A till will not close an unpaid basket, so this only
      catches a payload altered after it was written or bytes that rotted, which is what the totals
      check next to it is for as well
- [x] A counted drawer is now checked against the shop's own sales rather than against the till's
      word for itself. The one figure an owner acts on is the variance, and the whole of it came from
      the till: the till said what it expected, somebody counted, and the shop stored both without
      ever asking whether its own sales came to that. A till reporting a smaller expectation than it
      took hid a shortfall exactly. Each sale now carries what it left in the drawer, worked out from
      its tenders here (cash handed over, less change) rather than believed from a field, and the
      closed drawer list shows the shop's figure beside the till's when they differ. A till still
      sending sales differs honestly, which is why both are shown and neither replaces the other.
      A drawer holding sales from before this existed is answered with nothing rather than with a
      figure: those rows carry no cash, and reading that as an empty drawer would report every
      evening in a shop's history as disagreeing with its own till
- [x] `may_void_line` is enforced, and the flag stops being a promise nothing kept. Every operator
      record carried it, the back office offered it, and no code anywhere read it: a cashier could
      ring goods, take the cash, take the line off, and leave a smaller sale and no trace. Enforcing
      it on every removal would mean a supervisor for every double scan, which is a till a shop turns
      off, so the rule is the shape of the theft: free while nobody has paid towards the basket, and
      a supervisor's business once money is on it. A refusal is written into the trail as well
      (action 11), because the auth book records wrong PINs and allowed actions and had nowhere to
      put somebody simply not permitted. Action 10 got its words at the same time; it had been
      reading as "something this build does not know about" since the shelf rule went in
- [x] Both new back-office controls walked in a browser against Postgres, and the walk found a
      defect the tests could not: the shop's own figure for a counted drawer never reached the
      screen, because this layer's own shape for a drawer had no field to put it in. The server
      worked it out and the protocol carried it and the back office showed nothing. Now it says
      "Your own sales for this till come to 300.00, not 794.50" on a till holding a sale it has not
      sent, and says nothing where the two agree. The exempt classification saved from the item form
      and came back on the item's bytes in the database
- [ ] The print dialog is what stops any of that being automated: finishing a sale calls
      `window.print()`, which blocks the page until somebody dismisses it by hand. Everything up to
      the sale can be driven; the sale itself cannot
- [x] The way out of a device the shop will not take from, walked in a browser. A till holding a sale
      had its access withdrawn, said so with the number at stake, showed what it was holding with a
      mark over the bytes, and the back office computed the same mark (2742 8ee5) before taking them
      in: two devices, two copies of the wasm, one answer. The sale landed as one needing somebody to
      look, and pasting the same bundle a second time was recognised as the replay it is rather than
      counted twice. The shop holds two sales, one of them marked carried in by hand
- [x] Withdrawing a till's access is written down now. It is the loudest thing an owner can do from
      the back office, the device it stops may be holding sales nobody else has, and it was the one
      act that logged nothing
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
- [x] The general case is closed. An edit is read from the shop rather than from this device's copy,
      and carries back where the item stood when it was read; a save built on an older copy is
      refused with a conflict rather than merged, because a whole-item save cannot be merged and the
      older answer would win by accident. Withdrawing something reads fresh for the same reason

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
- [x] What a device allows now outlives the device. The till's own audit list was in memory and
      died with the process: a tablet restarted overnight could not say who opened the drawer, who
      allowed a refund, or who authorised a discount, which is the one question anybody asks after a
      variance. It is kept beside the counted drawers, pushed on the same footing as them (ahead of
      the catalogue, because it exists nowhere else), stored once per device count and clock, and
      read in the back office with both names: who did it and who allowed it. The clock is in the
      key beside the count because a device that dies between bumping the count and writing it down
      reuses it, and keyed on the count alone that record would be dropped as a duplicate
- [x] An open drawer survived the shop taking every sale in it. The critical log is emptied once the
      server holds everything in it, and the open shift is rebuilt by replaying that same log, so a
      till that synced mid-afternoon and then restarted came back with no drawer: the float the owner
      counted in, the change fetched from the safe, and the day's takings all gone, and the cashier
      met it at the evening count against a drawer that began at nothing. The closed drawers were
      given a home in the standing state for exactly this reason; the open one was not. The log is
      now held down until the drawer is counted, and emptied at the count as well as on the next
      acknowledgement. Deliberately not a partial cut: dropping the front of a log means rewriting
      it, and a crash inside that rewrite takes the unsent tail with it. What that costs, measured on
      files: 1,000 sales is 142 KB and a 2.4 ms boot, 5,000 is 712 KB and 10.8 ms
      (`cargo run --release -p openpos-bindings --example boot_cost`), against a log that holds the
      same day anyway whenever the internet is out
- [x] A sale still waiting for a receipt number was forgotten on restart. The count is read back from
      the log by asking what the lease block was doing, and a spent block stays active with its
      position one past its last number, so a sale that closed with nothing to number it recorded a
      position like any other and came back counted as numbered. The receipts had gone out blank and
      the shop was never asked to fill them in. It now reads the ticket's own number, and skips what
      the shop has already taken, which matters now that acknowledged sales stay in the log while a
      drawer is open
- [x] The count of sales waiting for a number is now cleared when the shop takes them, and not only
      worked out again at the next restart. It was live in memory, corrected only by a cold start, so
      a screen kept asking for numbers the shop already had until somebody rebooted the tablet
- [x] A drawer counted in the last moment before a crash is no longer lost. The count is a frame in
      the log and the queue it is sent from is the standing state, written a moment later, so a
      device that died in between came back with a drawer that replayed as counted and no record to
      send: the till refuses to count a drawer that is already closed, so the figure was gone for
      good. The queue is rebuilt from the frame on boot when the frame has no matching record, and
      the count now carries who counted it, so what is rebuilt names them. Drawer events went to
      schema 2 for that, with schema 1 still read and coming back with nobody named, which is what
      that build knew. Found by codex reviewing the truncation change
- [x] A shop that never counts its drawer lets the log go all the same. It used to keep every byte:
      roughly 145 KB of every thousand sales, until somebody pressed a button some shops never
      press. The drawer lived in the critical log and nowhere else, which is why, and the reasoning
      for that was right while the log is there: replaying the frames is what makes the drawer
      figure and the sales figure agree by construction.

      So the drawer is written down at the one moment the log is about to be dropped, and it carries
      the sequence it was folded through. The next boot starts from what was written and replays
      only what came after. That is the catalogue's own shape, a snapshot and the deltas after it:
      a checkpoint of the replay rather than a second opinion about it. Schema 19, with 18 frozen
      and a fixture of real version 18 bytes that still read whole.

      Written before the log is emptied on purpose. A crash in between leaves a fold and a log that
      still holds the same frames, and the sequence is what stops them being counted twice; the
      other order loses the day's drawer. There is a test for exactly that, and one for a morning
      of selling where nobody ever counts: five sales, the log empty, and the drawer coming back
      after a flat battery with its float, its movement and all five sales on it
- [x] A wrong PIN and the lockout it leads to are written down and reach the shop, beside what was
      allowed. One wrong PIN is a fat thumb; five on a Thursday evening is somebody standing at a
      till trying a colleague's, and only a shop looking at them together can tell. Kept apart from
      the permission enum, because getting a PIN wrong is not an action anybody may be permitted to
      take: the name on it is the button that was pressed, and the screen says so rather than
      claiming somebody's own permission covered it
- [x] A sign-in is written down with the rest, so the trail says who was standing at the till and
      not only who did the things that needed permission. Who was there when something happened at a
      counter is half of every question an owner asks about that evening, and it used to be
      inferable only from what somebody sold
- [x] `restore_line` is reached after all: resuming a parked basket rebuilds the cart line by line
      through it. It was listed as unreached from an earlier sweep that only looked for callers in
      the bindings, and it has one in the core. Checked rather than assumed
- [x] What a supervisor waived is something an owner can look at. A ceiling exists so that giving
      money away is somebody's decision rather than everybody's habit, which only means anything if
      the decisions can be looked at afterwards. The reason is on the customer's receipt already;
      it is projected out of the ticket as the sale arrives, so the question costs one query rather
      than a week of tickets decoded
- [x] A supervisor can allow one thing without the cashier signing out. The command existed, no
      screen sent it, and worse: for the two refusals a shop meets hourly, a discount over the
      ceiling and a price typed over the catalogue's, the authorisation did nothing at all. Those
      are stopped by the cart's ceilings, set when the cashier signed in, and the auth book was the
      only thing an authorisation reached. A supervisor typed their PIN, was told yes, and watched
      the discount refused again
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
- [x] The paper shows the tax by the rate it was charged at, and names the buyer when the shop has
      written them down. A basket of rice at fifteen percent and something exempt beside it printed
      one VAT number, which said nothing about which goods were taxed
- [x] What sold over a period, most first, read from the movements each sale wrote rather than from
      its payload. The question a shop asks before it orders, and it was not answerable: the till
      knew what it had sold and the shop only knew what it had taken
- [x] A lost or stolen device can be cut off. The store could do it since credentials existed and
      nothing could ask it to, so "unenrol the device" was an answer the shop had no way to carry
      out. Every credential that terminal holds stops, which matters because a renewal overlaps and a
      device that has renewed holds two. Two presses, because one press stops a working till dead
- [x] Looked at the repository methods nothing in the product calls. `append_change` turned out not
      to be one: it is an inherent helper both catalogue writers use. `has_sale` and `receipt_taken`
      are what the tests observe isolation and replay through, so they say so now rather than
      looking like leftovers a future sweep would delete
- [ ] Whether that paper satisfies the NBR's own form for a tax invoice is unverified. What is on it
      was chosen from what a customer and a shopkeeper need; nothing here has been checked against a
      primary source, and no claim of compliance should be made until it has
- [x] A supplier statement: goods in and money out between two dates, oldest first, with the whole
      balance beside it. What two people put side by side when the shop's figure and the
      distributor's disagree, which is the conversation the ledger exists for
- [x] The VAT figure says how much of itself is sales nobody has looked at yet. Counted rather than
      removed: goods may well have left the shop twice and a machine cannot know which, so it says
      what is uncertain and the person signing the return decides, the same stance as a drawer that
      came up short. Dealing with the queue entry makes the line go away
- [ ] The VAT figure still counts what was sold rather than what was collected. For a shop on the
      ordinary VAT basis that is right; whether any shop this serves is on a cash basis is unknown
      and unasked
- [ ] A sale on account against a name nobody wrote down is still keyed on the folded name, so two
      unregistered Karims still share an account. That is what the paper notebook does and what a
      shop that has written nobody down gets; writing them down is the answer and is now possible
- [x] A shop can see where its own numbering jumps. Receipt numbers are meant to run unbroken and
      the question an inspector asks is why they do not, and until now nobody could look: the numbers
      were in the sales and nothing put them side by side. Per till, per epoch, per prefix, so two
      tills counting from a hundred are not holes in each other. A gap is one of two things and the
      screen says so: numbers on a till that has not synced, which close by themselves, or numbers
      that went with a device that was wiped, which never will
- [x] A till whose clock cannot be believed is caught rather than filed. Nothing checked the time a
      sale said it was rung at, so a cheap tablet that had been off for a week and came back at 2010,
      or one running a year ahead, put its sales into the wrong day's takings and the wrong month's
      return, silently. Two impossibilities are held for a person now: a sale rung after the shop
      received it, and one rung before the device that rang it was enrolled. Everything between is
      left alone, because a month offline is what this product is for. An hour of tolerance each way,
      which is far more than any drift and far less than a wrong clock
- [x] The change handed back leaves the drawer with the note that came in. `record_sale` counted the
      tender and not the change, so a five hundred note for a basket of 494.50 made the drawer expect
      five and a half taka more than it held. Once a day that is a curiosity; every cash sale where
      somebody has no change is every evening of the year ending short, and a shop seeing that either
      stops trusting the till or goes looking for a thief who is not there. The cash row on the
      report says what stayed rather than what was handed over, so the rows and the figure under them
      agree. Found by ringing a sale with a note and reading the report
- [x] A discounted line on a receipt reads downwards. The row above the discount printed the line's
      own total, which already had the discount in it, so a customer read "one at 430.00, 445.05,
      less 43.00" and could make sense of none of it. The row now says what that many at that price
      comes to, in the same basis as the price beside it: before tax where the shelf price excludes
      it, with the tax in where the shelf price includes it. Found by rendering one and reading it
- [x] An overpayment on something that cannot give change is refused as it is typed, not only at the
      close. The cashier is looking at what they entered at that moment; at the close they are
      looking at a customer and have the whole tender to enter again. A discount given after the
      tender can still make an overpayment out of one that was fine, so the close checks it too
- [x] Change can only come out of cash. `change_due` said "only ever positive on an overpayment in
      cash" in its own doc and summed every tender, so putting six hundred on an account for a
      basket of four hundred and ninety-four told the cashier to hand back a hundred and five taka:
      real money out of the drawer, against a debt the customer was now also carrying. Change is
      capped at the cash tendered, and a sale over-tendered past that is refused at close rather
      than quietly dropped. A card cannot make change either, because that is a cash advance and not
      a sale
- [x] Typing the name of somebody the shop wrote down, instead of choosing them, is refused. The two
      are added up in different places: one against the person's record, the other against the
      spelling, so a shop ended up with a customer who owed for what they took and a phantom of the
      same name holding what they brought back. Found by running the account example end to end and
      reading the two lines it printed. The refusal names the person and the till offers them as a
      button, because the cashier is mid-sale with somebody waiting
- [x] Goods brought back by somebody who took them on account come off what they owe, and the day
      report says both sides rather than netting them. Three thousand on and three thousand back is
      not a day where nothing happened. The path worked already because a refund's tender is
      negative; what was missing was a test saying so and a report that could be asked which it was
- [x] An export carries the account book, entry for entry, because a payment is in no sale payload
      and a shop that arrives with its sales and none of what anybody owes it has lost the part it
      cannot rebuild. A debt can also be struck off with a reason, so a sale rung twice by a restored
      till can be corrected without recording a payment nobody made
- [x] An export carries the counted drawers too, with who counted each. A shop that moved machine
      and arrived unable to say a single evening was ever reconciled had lost the accountability
      record that is the entire reason one person counts a drawer and another reads it
- [x] A resolution says what was decided, not just that somebody decided. The queue was note-only:
      a shop that said "this was rung twice after the restore" kept the duplicate in its takings, its
      tax, what sold, its shelf figures and the customer's account for ever, with a note beside it
      that no arithmetic read. A sale can now be struck out, and everything that counted it stops
      counting it. Nothing is deleted: the sale, its bytes, its movements and its account entries all
      stay where they were, the figures filter rather than compensate, and it can be decided again.
      The decision travels in a bundle, so a restore does not put a struck-out duplicate back
- [x] A query that reads the sales either ignores the struck-out ones or says why it does not, and
      a test checks. Ten figures carried the filter and the eleventh, written next month by somebody
      who has never read the file, was one line from counting a duplicate for ever. The exemption is
      a line in the query itself rather than a list somewhere else: twelve reads carry one now, and
      each says what it is for. Checked by writing the forgetful figure and watching it fail
- [x] The three ways past that scan are closed, and each has its own test rather than a note saying
      it is known. It read one named file, so SQL moved to a file added next month would have been
      invisible: it reads every source under `server/src` from the directory now, and refuses to run
      if it finds fewer than it should. Text cannot see what a `format!` produces, so a query built
      at run time could name the sales, skip the filter and never appear: what reaches the database
      must be a written literal, which closes the injection door in the same move. And the table has
      four spellings Postgres accepts where the scan knew one, so `from public.sale` walked straight
      past a rule about a shop's money. Each of the three was reintroduced deliberately and watched
      to fail, and each failed on its own guard rather than by accident on another
- [x] A sale can be decided again. A strike-out takes a real debt off somebody's account and the
      entry leaves the queue, so getting it wrong used to be permanent with no screen to reach it
      from. There is a list of what was decided, an answer can be changed with its own reason, and
      every answer is kept: the latest is what the figures read and the rest are how a shop shows it
      changed its mind. Changing one carries what the screen saw, so two owners working the same
      list cannot overwrite each other, and a sale that was never held cannot be decided at all
- [x] The day report says why its two halves can disagree. The drawer figures are not adjusted by a
      sale struck out afterwards, on purpose: if that sale was rung and never happened the cash was
      never there, and the shortfall the counter wrote down is the evidence of it. The screen says
      that where the figures are, rather than leaving somebody to work out which number is lying
- [x] Who owes and what an account is made of are paged from a cursor rather than cut off. Five
      hundred accounts and two hundred entries used to be the ceilings, and past them a screen showed
      less than the truth with nothing to say so. Keyset, not offset: the owed list is ordered by
      what is owed, and a payment taken between two pages would make an offset skip somebody. Two
      people owing exactly the same, and two sales rung in the same millisecond, are the ties the
      tests cover, because those are what a cursor on one column alone gets wrong

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
- [x] The upgrade test now covers a device upgraded mid-shift. Drawer events went to schema 2 when
      the count learned to name who counted it, and the drawer is not stored anywhere: it is replayed
      from those events. A missing legacy path there is a shop that opens on Sunday, is told no
      drawer is open, and counts the evening against a float of nothing, or a till that will not open
      at all. Two devices from the old build are now opened by this one: one mid-shift, one holding a
      count the crash took, which comes back with nobody named
- [x] The routes a till talks to now say what they took. The back office logged what an owner did and
      the till-facing half logged almost nothing: a shop whose sales had not arrived had a log that
      could not say whether they ever reached the server, and a shop reading its own log could not
      see somebody guessing at enrolment codes. A line per push batch (not per sale, since a till
      syncs all day), per counted drawer with the variance in it, per block of receipt numbers, per
      device enrolled or credential replaced, and a warning per sale waiting on a person or
      credential refused. No credential is ever in a line. Every 503 names the line it came from,
      where sixty-nine call sites answered with a bare status and wrote nothing down. Read live off
      a real server and Postgres, not asserted in a test: enrol, lease, two push batches, a counted
      drawer with its variance, a carried-in sale, a withdrawn credential
- [x] Selling past the shelf is a shop's own decision, which the spec asked for at v1 and nothing
      implemented: `CartLimits` knew about discounts and price overrides and nothing knew about
      stock. Three answers, because there is no single right one: do nothing, sell it and say which
      line the shelf disagrees about, or refuse it until a supervisor allows it. Nothing by default,
      because a shop that has never counted holds none of everything as far as this system knows and
      a till that refused on that basis is a till that cannot sell. Enforced on the device, from a
      setting that travels with the shop's details, so it works with the line down. Scanning, keying
      an item and typing a quantity are all the same act and are all stopped. Refunds never are.
      What a supervisor allowed is on the customer's paper and in the trail, under its own code.
      Terminal state at schema 8, with 7 still read; every guard mutation-tested
- [x] The bytes older builds wrote are frozen and read back, which nothing did. Every legacy standing
      state names the current shape of everything nested inside it (the leases, the parked baskets,
      the people, the counted drawers, the customers, the credential, what was allowed), so a field
      added to any of those silently changes what seven legacy structs decode. The tests could not
      catch it: they built the legacy struct out of the same changed type and agreed with themselves.
      Now `core/tests/bytes_from_before.rs` holds the hex each version actually wrote, for the
      standing state at all seven versions, for a sale, and for the three drawer events, and asserts
      what a shop loses if they stop reading. Proved by adding a field to `CustomerV1`: version 5
      stops decoding and the test names the version
- [ ] The catalogue's own shapes are not frozen the same way: an item or a snapshot that stops
      decoding costs a till its prices until it pulls them again, which is a bad morning rather than
      a lost ledger, so they are left out on purpose
- [x] A till now asks the shop what the shelves hold, which nothing did. The catalogue carries a
      stock number that is whatever somebody last typed on an item record and never moves, so the
      rule above would have refused a whole day's trading in any real shop: the demo's shelves hold
      forty of everything and its catalogue records say none. Same question the back office asks, on
      a till-facing route, two hundred items at a time moving along the catalogue and wrapping, every
      five minutes, and only where the shop has asked to be warned or stopped. What this terminal
      sold and has not sent is added back on top, or the shelf jumps up while a cashier watches
- [x] The server is no longer the reason a lap is slow. On-hand was one transaction and three
      statements per item, so a till refreshing two hundred items made six hundred round trips: 140.5
      ms for two hundred against a database on the same machine, and far worse with any latency
      between them. The objection to fixing it was that a set-wide version of the query is a second
      answer to the same question, which is right, so it is not one: every expression is the one the
      single-item query uses, widened by a group-by and a barrier picked per item. A test runs both
      paths over a shop holding every shape the answer has to get right and compares them item for
      item, and it was broken deliberately to watch it catch a divergence. 3.5 ms for the same two
      hundred, forty times

- [x] The first lap of the shelf is taken at once now, and the ones after it slowly. The cadence was
      five minutes a window whichever lap it was, so a shop of eight hundred lines took twenty
      minutes to get round, and since a till says nothing about the shelf until it has been round,
      that was twenty minutes of a shop watching its counter and seeing the rule it had just turned
      on do nothing. Four windows half a minute apart is two minutes.

      The cadence was chosen against the database, and the database is not what it costs any more:
      the server answers two hundred items in three and a half milliseconds since it stopped asking
      one at a time. What is left is the shop's line, and four small requests once is not a bandwidth
      decision. The steady refresh is unchanged at five minutes, which is the figure that was
      actually argued about: how stale a shelf figure may be while a shop is trading.

      Broken back to the slow cadence for the first lap and watched to fail
- [x] The shop is shown back before the form offers to change it. The back office wrote the shop's
      details and never read them, so the name, the BIN, the address, the wallets and now the stock
      rule all opened empty: somebody who set a rule and came back tomorrow could not tell what the
      shop was doing without overwriting it, and an empty form saved over a shop's name. Read from
      the route a till reads, so what the screen shows and what a till obeys are one answer, and read
      again after a save, because the server trims the wallets and clamps the rule
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

## Speaking an item onto a ticket (2026-09-07, `docs/07092026_voice_lookup_plan.md`)
Found while reading the search path to see whether a cashier could say a product name. Neither of
these needs a microphone, a model or a browser, and both were wrong before anything was added.
- [x] A ticket could be started with a negative quantity. The mixing check compared the new line
      against the lines already there, so on an empty ticket it had nothing to disagree with and the
      line went on. A sale then totalled below zero, which passes the underpaid check with no tender
      at all, and the customer was handed the whole amount as change: cash out of the drawer against
      a refund nobody authorised, on a ticket with no original receipt, recorded as change given. On
      a refund the negation turned it positive and did the same thing mirrored. `set_qty` has refused
      this since the day it was found there and its comment describes this exact exploit; the sibling
      function, which is the other way onto a ticket and is reachable as `add` from both platforms,
      was left taking whatever it was handed. Two tests, both confirmed to fail with the guard removed
- [x] Bangla was being indexed a syllable at a time. The hasant, which is what makes a conjunct, and
      the nukta, which is what makes ড় out of ড, are not `Alphabetic`, so `normalise` turned both
      into spaces: "মিষ্টি" was stored as "মিষ" and "টি". Typed search survived it by accident,
      because it intersects its terms and the fragments of one word sit on the same item, and the
      Bangla test in that file passed for that reason and proved nothing. A shop could not search a
      word it could see on its own packet by any means that did not split it identically
- [x] The same word spelled two ways was two words. ড়ঢ়য় are written both as one codepoint and as a
      letter plus a nukta, and Unicode excludes them from composition, so its own normal form is the
      two-codepoint one and neither spelling is a prefix of the other. A catalogue typed on one
      keyboard and searched from another would say the shop's own stock does not exist
- [x] Bengali digits and Latin digits were separate shops. An item carries an English name with
      Latin digits beside a Bangla name with Bengali ones, so "৫" never found "Rice Miniket 5kg" and
      "5" never found "মিনিকেট চাল ৫ কেজি". Every demo item is affected
- [x] `core::voice` reads a transcript, with no new dependency and no audio anywhere near it. A
      lexicon of the Bangla a counter is spoken in, and a reader that produces the words worth
      looking an item up by, a proposed count, and a refusal in words when a number was said and
      will not be honoured. It offers a quantity and never applies one: the cost of a proposal a
      cashier ignores is a glance, and the cost of a quantity applied wrongly is the price
      difference, a wrong tax position, a stock figure that walks away from the shelf and an owner
      who stops believing the till. Eighteen examples and six properties, and every one of the
      seven refusal rules was taken out in turn and watched to fail a named test
- [x] Refused rather than guessed at, each with its own test and its own words on screen: an amount
      of money ("একশ টাকার চাল" is a hundred taka of rice, not a hundred bags, and this is how a
      large part of a counter here actually asks); a weight or a volume; part of a unit; a hali or
      a dozen; two numbers in one sentence with nothing saying which is the quantity; a number with
      no word saying what it counts; and a count above ninety-nine
- [x] The demo catalogue's own first item was the test case. "মিনিকেট চাল ৫ কেজি" is the name of a
      five kilo bag, and a till that read the five as a quantity would ring five bags at 2,150 for
      a customer buying one at 430, on the most ordinary sentence in the shop. The five is refused
      as a quantity and kept as a word to search on, because in that sentence it is what separates
      a five kilo bag from a two kilo one
- [x] Those words are resolved against the catalogue this device holds, read-only, and the till
      never rings from a spoken name. `Replica::weigh` scores rather than intersects, because
      typing is narrowing and speech is not: a cashier means every word they type, and a recogniser
      adds words nobody said and drops words they did. The typed search finds nothing at all for
      "ভাই একটু চাল দাও", which is a test
- [x] A word the catalogue has never heard of costs the match its confidence rather than being
      dropped for free. It is the strongest evidence there is that the till misheard, and treating
      it as free would make guessing free. Three confidence rules, each broken in turn and watched
      to fail a named test: half of what was asked for at least, twice whatever came second, and
      something distinguishing said rather than only words half the shop shares
- [x] A fourth confidence rule was deleted rather than kept. "Two matching words unless the one
      word names one item" could not be exercised: an item matching one word ties with every other
      item carrying it, and the runner-up rule had already refused. A rule whose presence and
      absence no test can tell apart is one the next person removes for the wrong reason
- [x] How common a word is counts only goods the shop still sells. The index carries withdrawn
      items so a refund can still find them, and counting those made the one item left on the shelf
      look like one of a crowd, so the till stopped being sure of the only answer there was
- [x] The safety property, which is what the whole shape of this rests on: an utterance corrupted
      the way a shop corrupts one, a word lost to a fan or a word gained that nobody said, may find
      nothing, or offer a list, or be less sure than it was. It may never become sure of a
      different item than the clean sentence pointed at. That is the failure nobody can see,
      because the screen looks exactly as confident as it does when it is right
- [x] One operation carries it to both platforms. `Command::Heard` is read-only and has its own
      field in the view rather than sharing the catalogue's, because "the cashier searched" and
      "the till heard" are different facts and a screen that cannot tell them apart cannot show
      what it thought it heard. Making it ring its confident candidate fails two named tests
- [x] The till screen takes a whole phrase in the lookup box that was already there, shows the
      words it used and the words it set aside, marks a match it is not sure of on that row only,
      and puts the offered quantity on the button, so what is about to be rung is what the cashier
      is looking at when they press it. The press is `Add`, the same one that list has always used
- [x] Verified in Chrome against the real server and the demo catalogue, not only in tests.
      "ভাই একটু চাল দাও" finds the rice where the typed search finds nothing at all. "মিনিকেট চাল
      ৫ কেজি" offers the rice and no quantity, with the core's sentence about weights on screen.
      "একশ টাকার চাল" refuses the hundred as money and marks the row not certain. "তিন প্যাকেট
      চাল" offers "3 × Rice Miniket 5kg" and pressing it rings 1,483.50, three at 430 plus VAT
- [x] Typed on purpose, and the microphone last. It is the only part that cannot be tested without
      a person in a room, so the whole of the understanding can be put in front of a shopkeeper
      with a keyboard and found wanting before anybody downloads ninety-four megabytes for it
- [ ] The recogniser and the model are not built. Neither can be until there is TLS: `getUserMedia`
      needs a secure context, and so does `navigator.storage.getDirectory()`, which means the
      storage layer already needs one on any real shop network and nothing had said so
- [x] Which model, decided against the published catalogues rather than from memory: the sherpa-onnx
      zoo holds exactly one Bengali transducer, `vosk-model-small-streaming-bn`, a Zipformer2 at
      94.4 MB under Apache-2.0. AI4Bharat's Bengali Conformer is 523 MB and needs exporting from a
      `.nemo` archive; their multilingual one is 2.56 GB; the Dolphin models are CTC and forfeit the
      hotword biasing a shop catalogue makes the biggest available lever. Not a Whisper derivative
      on purpose: an autoregressive decoder invents fluent text on unclear audio, and a fabricated
      product name is worse than a garbled one because it shows up as a good match
- [ ] And the weakness in that choice, named rather than buried: push-to-talk means streaming buys
      nothing, and the model chosen is the streaming one because no non-streaming Bengali Zipformer
      exists. So the Conformer gets measured rather than dismissed on size. The gate is hit rate on
      the shop's own product names, not WER: every number published for either model is clean read
      speech, and what decides this is whether the right item comes up out of forty product nouns
- [ ] Bengali is absent from the official Vosk model list. The model exists only as a Hugging Face
      repo and a release asset, which is thinner provenance than the rest of the zoo, so whatever is
      picked gets vendored with a pinned checksum rather than fetched by name
- [ ] `getUserMedia` needs a secure context, and so does `navigator.storage.getDirectory()`. Which
      means the OPFS backend already needs one and nothing has ever said so: development runs on
      `127.0.0.1` and `localhost`, which are secure contexts, and a tablet reaching the shop's server
      at `http://192.168.1.x:8080` is not. The TLS terminator below is a prerequisite of the storage
      layer on a real shop network, not only of anything with a microphone
- [ ] Weight and volume quantities cannot be taken from speech, and the blocker is not speech.
      `Item.unit` is prose, defaulted to `"Nos"` and typed by hand, so nothing in the system says
      whether an item is sold by count, by weight or by volume, nor how many grams are in one of it.
      "৫০০ গ্রাম" against a 500 g packet and against loose goods are a thousand times apart and the
      till has no fact that tells them apart. Structuring `unit` touches the protocol, the disk
      schema, the server and the back office, and is its own piece of work

## Open, and named rather than left implied
- [x] Token renewal: `/v1/renew` trades a working credential for a fresh one, authenticated with the
      one being replaced. The old one lapses after a day rather than being revoked, because the reply
      can be lost and a till whose only credential vanished mid-request is a shop offline until
      somebody re-enrols the tablet by hand. Renewing never extends a deadline
- [x] A full day's drain is measured on the server side: 300 sales in 12 batches of 25 took 0.31 s
      against Postgres in release, 1.04 ms a sale, and 1500 took 1.58 s at 1.06 ms a sale. Five times
      the work, the same cost each, so nothing bends upwards with the size of the day. What that
      leaves is the line: a shop adds one round trip per batch, twelve of them for a day like that,
      which is what decides the wait rather than the server
- [x] What a sale costs to write down before the cashier is told it is done, measured now that a
      native durable store exists: 4.6 ms a sale on files with a flush each, against 0.002 ms for
      the arithmetic alone. Flat from fifty sales to five hundred. Milliseconds rather than tenths
      of a second is what decides whether a queue moves, and the shape rather than the figure is
      what transfers to a tablet's flash
- [ ] Not measured, and not measurable on a desk: the same drain over Bangladeshi mobile data, and
      the same flush on a cheap Android tablet, which is slower than any desk and is where the
      figure decides anything
- [x] An export describes one moment. Every append-only read is cut at the database's clock when the
      drain starts, so a sale that lands mid-export is left out of it whole: its stock movements and
      its account entries go with it, rather than the export catching some tables and not others and
      restoring a shop with stock that moved for no reason and a debt with no sale behind it. The
      catalogue is cut by the shop's own sequence. Re-running still converges, and now what it
      converges from is consistent rather than merely incomplete
- [x] An operator can actually take a backup. The export had existed since the week it was needed
      with tests and no caller, which made it a library rather than something a shop can do:
      `openpos-server export <shop>` writes the bundle to stdout, and the logs moved to stderr so a
      redirect gives a file that reads back
- [x] A till renews its credential. The route, the overlap and a reply carrying the shop's own
      policy had all existed since the week they were designed, and nothing ever called them: every
      device would have stopped working exactly one year after it was enrolled, with a screen saying
      the shop was refusing it and a shop with no way to fix it but to re-enrol every tablet by hand.
      Found by sweeping the routes for ones no client can reach
- [x] A backup carries the people who may stand at a till, the people the shop buys from, and what
      the shop prints at the top of a receipt. Without the first, a restored shop could not sell at
      all: the tills enrolled, the catalogue arrived, and nobody could sign in. Without the last it
      printed a tax invoice with no BIN and no address. No PIN travels in the file, because four
      digits behind any number of rounds is a few thousand guesses to whoever holds it, so each
      person arrives with a key from the importing machine's own generator and the import says out
      loud that a PIN has to be set. A second run leaves anybody already there alone, or it would
      lock the shop out of its own tills
- [x] An operator can put a backup back: `openpos-server import` reads a bundle on stdin, keeping the
      shop's own id because the tills still hold sales carrying it, or `--as <shop>` for a copy. The
      restore half had been library-only with tests and no caller since the week it was written
- [x] A bundle carries the deliveries, the supplier payments, the stock counts, the corrections and
      the trail of what was allowed. Before this a restored shop knew what people owed it and not
      what it owed its suppliers, worked its shelf figures out from the movements with no count
      barrier behind them, held corrections with no reason attached, and could answer "who allowed
      this" about nothing before the move. Checked live: two databases, one exported into the other,
      agreeing to the poisha on what is owed and on what a counted shelf holds
- [ ] The rows that are not append-only, the terminals and people and suppliers and customers, are
      still read as they stand rather than as of the cut. That is what a restore wants, and it means
      a bundle mixes one moment's ledgers with another moment's lists
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

- [x] The repository has the licence it declares. Every crate said `AGPL-3.0-only` and no LICENSE
      file existed, so what the project actually grants anybody was undefined and every packaging
      tool said so. The text is the GNU AGPL v3 verbatim, checked section by section against what
      that licence contains, and the two entry points carry the notice the licence asks for
- [ ] `wasm-pack` still says the bindings crate has no LICENSE: it looks inside the crate rather
      than at the repository root. Nothing is published from here yet, and copying a legal text into
      four crate directories to quiet a warning about something the project does not do is not worth
      the four copies to keep in step

- [x] A shop can sort its shelves in its own words, and read a month's selling by them. Item
      categories were a v1 spec line and nothing carried them: an owner with eight hundred items read
      the top twenty of one long list and learned nothing about whether the rice moved. The shop's
      own words rather than a list this project chose, because a grocer, a pharmacy and a hardware
      shop do not sort the same way. Suggested from what the shop already uses, so a second bag of
      rice goes under the same word as the first. Quantities are not added up inside a group: a kilo
      and a bar of soap are not two of anything. Terminal state 12, snapshot and catalogue batch 4,
      with the item frozen again. Walked live: the owner sorted milk under Dairy and a book under
      Stationery and the till received both, with what each is for tax

- [x] A receipt somebody brings back to the counter can be looked up. The shop held every sale it
      had ever taken and had no way to answer the question it is actually asked: "you charged me
      twice", "I did not take this". The repair queue answers which sales went wrong and the day
      answers what was taken; neither answers what was on this piece of paper. Read out of the bytes
      the till committed, with the same crate that priced the sale, so a ten percent discount comes
      back as the taka that came off rather than as a rate. Both sales when two carry one number,
      because that is the case somebody comes in about, and the money given back against it since.
      Owner only: a tablet on a counter is not a place to look up what anybody bought

- [x] What a day made, not only what it took. A shop knew its takings and nothing anywhere could say
      its margin, which is the question that decides what to put on the shelf: a sack of rice that
      moves twice a day at four taka of margin is worth less shelf than soap that moves twice a week
      at forty. The cost is frozen onto the line with the price, so a supplier putting his price up
      next month does not rewrite a figure the owner already acted on. Turnover before tax, because
      the tax was never the shop's money. Sales with anything uncosted on them are counted apart and
      their turnover is left out of the figure as well: half a margin read as a whole one is worse
      than none, and a shop that has never entered what it pays would otherwise read a margin equal
      to its whole turnover. Sale schema 4, terminal state 13, parked baskets frozen again. Walked
      live: 86.00 made on 774.00 of selling against 688.00 of goods

- [x] A delivery says what the shop pays, and the catalogue keeps it. The margin shipped an hour
      before this could only ever answer "I do not know": the item form had no box for a cost and
      booking goods in recorded the price on the challan against the delivery and nowhere else, so
      nothing ever set what an item costs. A delivery now writes it onto the item, which every till
      pulls with the rest of it, and an owner can type it as well. The last delivery's price plainly,
      because that is what a shopkeeper means by what a thing costs and an average nobody can
      reproduce from their own papers is a figure they will not trust. A line booked with no price is
      somebody recording goods rather than a price change, and leaves it alone. Walked live: a
      delivery at 395.00 moved the cost off the demo figure, and the same two sacks then showed a
      loss of 16.00 at ten percent off, which is the thing an owner needs to see

- [x] A shop that loses the device running its back office can get back in. Every enrolment code
      came from the back office itself, and the only owner's code a shop ever had was printed in the
      log the first time the server started: a year later that is gone, and the database full of its
      takings could not be looked at by anybody. Found by walking the screens in a browser rather
      than by reading them: the list of devices offered "Code for this till" for the back office
      too, because the list never carried which device was which and the button asked for a till's
      role every time. The list now says, the button offers the right one, and an operator on the
      machine can mint one with `openpos-server code <shop>`
- [x] The back office panels of the last few commits walked in a browser against Postgres: a receipt
      looked up by its number showing 2 Nos x Rice Miniket 5kg (less 86.00) at 890.10 with the cash
      that paid it, and the day reading "Made -16.00 on 774.00 of selling before tax, against 790.00
      the goods cost you" after a delivery at 395.00 moved the cost

- [x] A till enrolled for the first time now syncs without being reloaded. Walking a fresh device
      showed it sitting at "0 numbers" and "nobody has been added to this shop yet" for as long as
      anybody watched: the app asks the worker for the sync loop as it boots, and the worker refused
      every command before a till was open, so on the one boot where enrolment comes after the ask
      the loop was never armed. A shop's first till, and every new one after it, looked broken until
      somebody reloaded the page. Which commands can be answered before a device knows who it is is
      now one rule with its own tests rather than the order of ifs in a file. The bundle mark moved
      with it: its own comment said it needed no till, and it was behind the check
- [x] The permission on taking a line off a paid basket walked in the real till: a cashier is
      refused with "That needs a supervisor", the supervisor allows it from the same screen without
      the cashier signing out, and the line comes off

- [x] What to buy, which is the question on the way to the wholesaler and the one the shop could not
      answer. It knew what had sold and it knew what was on the shelf and nothing put the two
      together: a list of what sold most is not the answer, because the thing that sells most is
      usually the thing that is still there. How many days each shelf lasts at the rate it has been
      selling, shortest first, over the same window and the same items as the report above it so the
      two halves cannot be about different weeks. It says nothing about how much to order: that
      depends on when the supplier comes and what is in the drawer. Built from the two answers the
      shop already had rather than a third query, so the careful part (what a count does to an
      on-hand figure) has one implementation. Walked live: rice, 57 left, about 152 days

- [x] What is not moving, which is the same question from the other end and the one that decides
      what to stop buying. A small shop's cash is on its shelves: something that has not sold in a
      month is money it cannot spend on what does sell, and the list of what sold is by definition
      the list without those things on it. Valued at what the shop paid rather than what it hopes
      for, because what it hopes for is not money it has. Something it has never priced is still
      listed, worth nothing anybody can state, so the total is never quietly smaller than the truth.
      The stock answer now says whether it covered every item or a page of them, because a figure
      added up from two hundred of eight hundred items reads as a figure for all of them. Walked
      live: six things, 26,240.00 of the shop's money, biggest first
- [x] One script stages the apps, because doing three of its four steps is worse than doing none: an
      app rebuilt against a stale core boots, looks right, and fails on the one command the new core
      added. That cost two browser sessions spent looking for a bug in a screen that was fine

- [x] The drawer count prints. Everything on that report was on the screen and nothing else, and the
      print stylesheet shows only a receipt, so at the one moment of the day when a shop most wants a
      record nobody rewrote, a cashier copied the figures onto a slip by hand. Laid out by the same
      crate that lays out a receipt, so a thermal printer, the browser and the Android build produce
      one slip rather than three: the till, the hour, who counted, every kind of money with a note on
      the ones that never reached the drawer, what it should hold, what was found, and short or over
      in words rather than a minus sign. Two name lines at the bottom, because a count is where money
      changes hands
- [x] The print button on that report pressed in a browser, on a drawer still open and on one
      counted: "DRAWER COUNTED ... SHOULD HOLD 300.00 ... Counted 295.50 ... Short by 4.50" with two
      empty name lines under it. Pressing it mid-shift showed the slip saying "Counted by Demo Owner"
      when nobody had counted anything, which is a slip saying something untrue about a person by
      name; open drawers now say who printed it

- [x] A refund is rung against the receipt in the customer's hand, so every check written for refunds
      is finally reachable. The core has taken the original receipt since it was written and the till
      screen never asked for it, so every refund a shop could ring arrived at the server with nothing
      to check it against: whether the sale exists, whether it has already been refunded, whether
      more is coming back than went out. All of that was written, tested and dead. The cashier is
      asked for the number, and can say they have not got it, because somebody who lost the paper is
      still owed their money. Walked live: the first refund against T3753-000001 was taken, the
      second was held with "receipt T3753-000001 was rung for 49450 and 98900 has now been refunded
      against it"

- [x] Goods gone can be written off from a screen. Manual stock corrections were built in the week
      they were needed, with a reason required and refused when blank, and no screen and no binding
      could reach the route: a shop that dropped a bottle of oil had two ways to move a stock figure,
      sell it or count the whole shelf, and nowhere at all to say what happened to the difference.
      Found by listing the server's routes and asking which of them any app calls. Walked live: two
      bags spoiled in the rain, and the shop's own log says "stock corrected qty_milli=-2000"
- [x] `catalogue/delete` is called now: the back office's own delete path reaches it, and an item
      the shop has traded is withdrawn rather than deleted. This entry was stale, and finding that
      out by hand is the thing the reachability guard below now does every time the tests run

- [x] A customer's account prints: the khata page they take away. A shop here sells on account all
      day and settles weekly, and the answer to "how much do I owe" was a number on a screen the
      customer cannot carry home. A figure somebody cannot check against their own memory is a figure
      they argue about at the counter. Every amount on it is what the shop sent; the screen passes
      only what a clock makes, one date per line, and a mismatch in that count is refused rather than
      paired onto the wrong days. The running total is added up by the crate that lays out receipts,
      so the paper cannot disagree with itself. The back office had no print surface at all until
      now: what an owner could put on paper from there was a screenshot
- [x] Pressing that button found the account being eaten by the sync loop: it was read out of the
      last applied reply, and the loop applies something every couple of seconds, so by the time
      anybody pressed print the account had been replaced by a catalogue page. Kept in its own field
      now, with a test that lands a sync round between reading and printing

- [x] A restored shop can still say what it made and what its drawers took. Every sale in a bundle
      came back with what it left in the drawer and what its goods cost set to nothing, so a shop
      that restored from its own backup was told its whole history made no money and that no drawer
      it ever counted could be checked against its own sales. Both are read back out of the bytes
      the till committed, like the tax rows beside them, so a bundle written before either figure
      existed restores with them: the lines were always in there
- [x] A backup taken by an older build restores whole. The import read every payload with only
      today's sale format, so a bundle from last month came back as sales with no tax rows, no
      waivers and no refund named: the sale survived and everything read out of it was gone. It now
      tries the formats this build knows, newest first

- [x] Any paper this till lays out can be handed to a thermal printer, not only a receipt. The
      byte path rendered the last sale and nothing else, so a shop with a printer and an Android
      till could print what it sold and not what it counted or what anybody owed. It encodes the
      lines as they were laid out rather than rendering the record again, because those lines
      carried the screen's own clock and names and a second rendering would print a different page
      from the one somebody just read. Reachable from the C ABI without a change, since that is one
      JSON door
- [ ] Nothing yet writes those bytes to an actual printer. The browser prints through its own
      dialog, and the Android till that would open a socket has still never been built or run on a
      device

- [x] A shop can say how much anybody may owe it. It could see what each person owed and had no way
      to say "not past this": the only control was a cashier remembering a number at a counter with
      the customer standing there, and a shop whose cash is on somebody else's shelf is the ordinary
      way a small one dies. Per person, in the back office, and zero is no cap, which is where every
      shop starts. The till refuses a sale on account that would take somebody past it and a
      supervisor standing there can allow that one, like a basket past the shelf; a supervisor at
      the till needs nobody. Measured against what the shop last told the device, which on a till
      that has not synced since morning is the morning's figure: that is the honest position for a
      device that has to keep selling with the internet down, and the refusal says what it knows.
      Terminal state 14, customer wire appended, migration 0034
- [x] The cap walked in a browser, and the walk found it doing nothing. The list of people arrives
      at a till with the cap on it and this layer dropped it one line before the till was told, so
      the back office could set a limit, the screen could show it, and no till anywhere would stop a
      sale. The refusal also had no way through: the tender was sent without the supervisor prompt,
      so a cashier was told no and offered nothing. Both fixed and walked: "Karim, flat 3 owes 240.60
      and you allow 300.00: this would take them to 735.10", then "Demo Owner allows it", and the
      sale goes on. The picker now reads "owes 240.60 of 300.00", so a cashier sees it coming

- [x] Everything the shop says about an item, a person and itself is checked across the crossing
      where three fields were lost in one day: the tax classification, the shop's own sorting, and
      the cap on what somebody may owe. Each time the tests on both sides passed, because both sides
      were right and the middle threw the field away. Three tests now build a wire record with every
      field set to something a default cannot produce, apply it, and compare what the till holds
      field by field, so the next one fails on a laptop rather than in a shop

- [x] And the same guard the other way: what a till writes down reaches the shop. That direction
      matters more, because a drawer is counted once by a person at the end of an evening and an
      item written down at the counter is the only record of a price somebody sold at: a field
      dropped on the way out is a field the shop never had. Two tests carry a counted drawer and a
      till-written item across and compare what arrives, figure by figure

- [x] Prices can be moved together, which is what a shop does when the wholesaler moves. One item at
      a time through a form is an afternoon nobody has, so the prices stay wrong and the margin goes
      quietly. A percentage over whatever the list is showing, read as a list of old to new before
      anybody agrees to it, and each one landing on the nearest taka because that is what goes on a
      shelf label. Written one at a time through the same door a single correction goes through,
      each read fresh first: a price somebody else changed while the list was being read is refused
      rather than overwritten, and the screen says how many of them that was. Walked live: "Rice
      Miniket 5kg 430.00 to 452.00", pressed, and the shop now serves 452.00 to its tills

- [x] No legacy shape names a shape that is still growing. Three of them did: two standing states
      held the live item, and one held the live parked baskets. Correct today and a landmine
      tomorrow, because the next field added to either would silently change what those bytes claim
      to be, and the symptom is a till that cannot open its own ledger after an upgrade. Frozen
      copies of both as they stand, and a test that reads wire.rs and fails on any legacy struct
      naming one of the shapes known to grow. The list of those shapes is in the test, so adding a
      field to a new one means putting it on the list, which is what makes somebody freeze a copy
- [x] Codex was out of credits and is not any more. Three external reviews since: the day's eleven
      commits, the shop boundary, and the path that keeps sales safe on a device. They found one
      thing a walk could not have, four inside the shop boundary, and six around storage and money,
      and they said plainly what they had read and found sound, which is worth as much: a review
      that only ever finds things is a review nobody can calibrate

- [x] The backup sidecar exists, and takes a backup nobody has to remember. A shop that self-hosts
      has one copy of everything it has ever sold, in one volume, on one machine: the export has
      existed since the week it was needed and nothing ran it. The sidecar is the same image, run as
      a loop: export to a part-file, read it back the way a restore would, and only then give it its
      real name and drop the oldest. A truncated bundle looks like a whole one until the morning
      somebody needs it. `openpos-server verify` is what reads it, needs no database, and exits
      non-zero saying which line stopped it, so a backup can be checked on the machine it was copied
      to. Walked live: 19,694 bytes holding 7 sales, 9 catalogue changes, 17 movements, 3 account
      lines, a counted drawer, 2 people and a customer; the same file cut in half is refused

- [x] The TLS terminator exists, behind a profile so a bench stays two containers. What crosses a
      shop's wifi otherwise is a bearer credential and the day's takings, in the clear, on a network
      whose password the delivery man also knows. Caddy in front, one hop declared on both sides
      because the server rate limits by the caller's address and behind a proxy every request
      arrives from the proxy. Walked live: the whole thing up behind it, the till and the back
      office both served over https, and a certificate Caddy issued itself
- [x] And running the sidecar for real found what a test could not: the image runs as a non-root
      user, the named volume takes its ownership from the image the first time it is mounted, and
      nothing in the image made the directory. The sidecar started every night, wrote nothing, and
      said so into a log nobody reads. It also crash-looped when the shop id was missing, which
      buries the line that says why. Both fixed, and the first real run wrote 7,768 bytes and read
      them back

- [x] A till says when it last reached the shop, and shouts when that was a while ago. A browser
      freezes a hidden tab and stops its worker with it, which is the failure this design is
      arranged against: the status line then keeps saying whatever it said when the freezing
      started, which reads as a till that is fine. The header now carries the hour of the last round
      that got through, aged by a clock of its own, and says "nothing has reached the shop for N
      minutes" past five. A tab coming back to the front syncs at once rather than waiting for the
      round a browser may have stopped. Walked live: "reached the shop 13:18:57" in the header; the
      warning past five minutes is the same figure over a threshold and was not separately walked

- [x] A shop can bring in the list it already has. Until this, a shop with eight hundred lines was
      being asked to type them into a form one at a time, which is the answer that ends the
      conversation: every one of those lines is already in a wholesaler's price list or an export
      from whatever they ran before. A CSV is read in the browser, matched against what the shop
      already sells by code and then by barcode, and put on the screen in full before a single row
      is written: what will be added, what will be corrected, and every row nobody can read named by
      its line number with the reason in words. Each row is then an ordinary save, so one the shop
      refuses is refused for its own reason and the rest still land
- [x] And walking it found the defect the matching existed to prevent. A back office one minute old
      read a file, matched it against a copy of the catalogue it had not pulled yet, called every
      row new, and left the shop with two "Rice Miniket 5kg" under one code: one with the stock and
      the other with the sales. The screen said "new" and meant "I have not looked". A file is now
      not read at all until this device has pulled the catalogue to the end, which it learns from
      the sync round itself: a pull says outright whether more is waiting, a wait says the driver
      has nothing left to do and is only believed when nothing is failing. Walked live twice: the
      refusal on a device seconds old, then the same file matching three demo items by code and
      adding two, with 9 items behind 12 changes and the stock on the corrected ones untouched

- [x] Deleting an item is reachable, and refused for anything the shop has traded. The route had
      been written, tested and shipped since the catalogue existed and nothing in either app could
      call it: the fourth rule found this way, and the route-versus-app audit now finds none left.
      Worse, it deleted anything asked for. A deletion is a tombstone every till obeys on the next
      pull, and for an item the shop has sold, taken in or counted it takes the name off figures
      still in the books. The shop now asks whether anything has ever happened to the item and
      refuses in words that name the act to use instead; the button is offered only on something
      already withdrawn, and takes two presses. Mutation tested: the guard removed, the named test
      fails. Walked live both ways: an imported line nobody had traded deleted and gone from this
      device's own copy within half a minute, and a delivered item refused with nothing written
- [x] And a message on the back office had a life of fifteen seconds. The till list refreshes on a
      timer, through the same helper every button uses, and that helper clears whatever is on the
      screen before it starts. So the shop's refusal, the one sentence saying what to do instead,
      was wiped while somebody was still reading it. The timer's refreshes are quiet now. Walked:
      the refusal still on the screen twenty seconds later

- [x] A till can answer "what does this cost" without ringing it. The question a cashier is asked
      twenty times a day, and the only way to answer it was to put the thing in the basket and take
      it off again: a line on the trail saying somebody voided something, and a supervisor's
      permission once the customer has started paying. A scan in this mode answers from this
      device's own catalogue, so it works with the line down, which is when a missing shelf label is
      likeliest to be the only other source. The figure is worked out by the same arithmetic that
      would ring it, not by adding a percentage on the screen: a price quoted across the counter is
      one the shop has to honour, and a screen computing its own would be the untested one.
      Mutation tested: quote the net instead of the gross and the named test fails
- [x] And walking it found the first thing wrong with it. A withdrawn item answered "no item in the
      catalogue has that", about a thing the till was holding and could name, which sends a cashier
      hunting for a barcode that is perfectly good. It now says which item and that the shop has
      stopped selling it, in the same words the scanner path uses. Walked live: 212.75 each
      including 27.75 tax with the basket untouched, the withdrawn item named, and "Ring one up"
      landing the same figure on the ticket

- [x] Four things wrong with the import, found by reviewing it against what a shop's own file
      actually looks like. A byte order mark, which is what Excel writes at the front of every CSV
      it saves as UTF-8, made the first heading unreadable and the shop was told its own export was
      not a catalogue. Semicolons, which Excel writes wherever the decimal separator is a comma, and
      tabs from anything pasted out of a sheet, were read as part of the text. Two rows under one
      code created two items, and which one a scan rings is whichever the index kept. And the rate
      for a row whose file says nothing about tax was borrowed from the form above, so an owner who
      had cleared that box would have imported a whole catalogue at nothing per cent and
      under-declared every sale of it. That rate is its own box now, refused when it is not a
      number, and the preview says which rows will get it. Walked live with a file carrying all
      three of Excel's habits: two written, the repeated code refused by line number
- [x] A shop can take its own list out, in the shape the import reads back. The half that makes
      bringing one in safe to use on a price rise: take the list out, change the column in the
      spreadsheet they already know, bring the file back, and every row carries its code so it
      corrects rather than adding a second copy of the shop. Written from this device's own copy, so
      it works with the line down, and with a byte order mark on the front because without one Excel
      renders a Bangla name as mojibake and the shop's own list looks broken. The round trip is not
      a claim: a test runs the writer into the reader, and it was walked live by feeding a file the
      shipped writer produced back into the panel, which matched both rows to items the shop already
      sells. Untested: the browser's own save step, which did not produce a file in this automation
      profile
- [x] And two messages that could disagree on that screen. A refusal left from the last press sat
      beside the next success, because only some of these acts cleared what was there first
- [x] A price a long way from the one the shop holds is put in front of somebody before the import
      writes it. An import can reprice eight hundred lines in one press and the preview shows the
      first twenty: a formula dragged one row too far, a column of poisha read as taka, an extra
      zero typed at midnight, all look like an ordinary row on the screen and like a shelf nobody
      can explain in the morning. Anything at least double or at most half of what the shop holds is
      listed first, with what it was and what it becomes. Not refused: a shop that doubles a price
      has every right to. Walked live: 430.00 becomes 4,300.00 named by line number, with the two
      ordinary rises beside it unflagged

- [x] The five minute outage, walked at last, and the warning it was written for could not fire.
      "Reached the shop" was set on any round the loop called successful, and a round that decides
      to wait is one of those: a till backing off after a failure decides to wait every two seconds,
      so the header refreshed the time of contact all the way through the outage it exists to make
      visible. Only a round that exchanged something counts now. Walked: contact at 15:27:29, server
      stopped at 15:27:30, "nothing has reached the shop for 5 minutes" in the warning colour at
      15:32, and back to "reached the shop 15:36:45" on the first round after the backoff elapsed.
      Worth knowing: after a long outage a till waits out its backoff, up to about three and a half
      minutes, before it notices the shop is back. A tab brought to the front syncs at once, which
      is the way a cashier shortens it
- [x] `openpos-server help`, and the same list beside anything the command line cannot read.
      `code --tenant <id>` is the shape every other tool takes and it answered "--tenant is not a
      shop id" while saying nothing about what would have worked

- [x] The till speaks Bangla. The spec has had "i18n, English and Bangla" in v1 since it was
      written and every word on every screen was English; the only Bangla anywhere was the item
      names a shop typed in itself. The operating surface of the till is now translated and switched
      by one button in the header, named in the language it switches to, kept per device because the
      tablet on the counter is read by whoever is standing at it
- [x] And the half that mattered more: the refusals. A cashier needs their own language exactly when
      something is refused, and those sentences are built in the core. Each one now carries a frozen
      code and its figures named and formatted apart from the words, so a screen can say it without
      matching on English that somebody may improve tomorrow. Three tests hold it together: the core
      freezes the list and writes it to apps/shared/refusals.json, a second test refuses a code the
      list does not hold and a list entry nothing produces, and the JavaScript fails when a code has
      no words in every language or when a translation drops a figure the English names. Walked
      live: "এই বারকোডের কোনো পণ্য তালিকায় নেই" for an unknown barcode and "ভুল পিন: আর 4 বার চেষ্টা
      করা যাবে" for a wrong PIN, the figure carried through
- [x] And the rest of the till with it: the drawer, the reports, the parking, the account panel and
      every sentence the screen says when something has gone wrong. The three tender kinds every
      shop has are said in the shop's language and a wallet keeps the name the shop gave it, because
      "bKash" is a name rather than a word to translate
- [x] Which found one more thing. On a sale the customer overpaid, the button that means "they
      handed over exactly this" read "Exact (-287.25)" and quietly took the overpayment back out:
      the drawer came to the same figure, and the receipt then said they paid the exact amount when
      they had handed over a five hundred note and taken change. It is off on an overpaid sale now.
      A refund is the other way round and is what the negative is for
- [x] And the last English on the till: what the screen itself refuses before the till is asked (a
      quantity with four decimals, a float nobody counted, cash moved with no reason), the panel for
      carrying sales off a device the shop has refused, and the sign-in screen of a shop with nobody
      in it yet. Walked: the only Latin left on the screen is the product's name, the button offering
      the other language, and the name the shop typed for its own owner
- [x] The back office speaks Bangla as well, all twenty-four panels of it, with its own language
      setting: the two apps share an origin, and a shopkeeper may want the counter in Bangla and this
      in English. What is wrong with a row of a shop's own spreadsheet is now named rather than
      worded, for the same reason the till's refusals are: the screen reading it may be in Bangla.
      Walked: the only Latin left is the product's name, the button offering the other language, the
      column names a CSV must use, and the words the shop typed itself
- [x] The repair queue was written in numbers only this repository can read. An owner deciding
      whether a sale is real was told "the till stored 21275" and "rung at 1788600000000", and how
      much came back was in thousandths. Money is money now, quantities are quantities, and a clock
      that is wrong is described by how far out it is: the shop's own hour is the screen's to know
      and not the server's

- [x] And an item nobody could price is refused where it is written. A rate over a hundred percent
      stored in the catalogue is refused by every till when it reads the page of changes it arrives
      in, and a till refuses the whole page: one number typed in the item form, or in the box that
      gives a rate to CSV rows whose file says nothing, would stop every device in the shop from
      seeing any price change at all, with nothing at either end saying why. The shop refuses it now,
      naming the consequence, and both screens refuse it before sending. Mutation tested. Found while
      reading what an external reviewer went looking for before it ran out of credit
- [x] The back office's own refusals speak Bangla too. Every "say why" and "that is not a date" on
      that screen was still English after the panels around them were translated

- [x] The papers speak Bangla too: the receipt a customer takes away, the slip that goes in the
      drawer with the cash, and the account page a neighbour is handed when they settle up. The core
      lays them out and asks for every label by name, defaulting to English and holding no
      translations of its own, so the thermal path is handed nothing and prints what it always did:
      no ESC/POS code page carries Bangla. Frozen the same way the refusals are, with the core
      writing the key list out for the screens. Walked live: a whole receipt in Bangla, নগদ included
- [x] Why a sale is being held now reads in the shop's language. The queue is where an owner is asked
      to judge a sale, and it was one English paragraph: the reason was stored as prose, so there was
      nothing for a screen to translate against. The reason itself is stored beside the words as
      postcard, and the bindings turn it into a name and its figures. A sale held before the column
      existed still shows the sentence, which is what an operator read at the time. Walked live: a
      till whose clock ran two hours fast, held on arrival, and read back in Bangla
- [x] And walking that found the classic i18n mistake in my own work: the gap in time arrived as a
      count, a unit and a direction, and all three were poured into the sentence. The screen read
      "1 hours after" in the middle of a Bangla paragraph. The direction picks the sentence and the
      unit picks a phrase now, so nothing crossing the boundary is an English word pretending to be a
      figure

- [x] And the protocol version with it, which I had not bumped. The codebase's own rule says a field
      added to a reply makes every older body undecodable, so the version is what tells the two sides
      which shape they are looking at: a back office one release behind would have asked for its
      repair queue and been handed a shape it could not read, showing an error where the queue should
      be. Version 3, with the old shape kept and served to anything older. The test needed two held
      sales to prove it: with one, the extra byte lands at the end where a decoder ignores it

- [x] The number on the box finds the item. A label that will not scan is an ordinary afternoon: the
      cashier reads the barcode off the box and types it into the same place they type a name, and
      the index behind that search holds names and codes. The shop's own barcode found nothing, which
      reads as a shop that does not sell the thing in their hand. Both screens use that search, so it
      is fixed for the till's lookup and the back office's shelf list at once

- [x] And the second place a shop is shown that a sale is held: a receipt looked up by its number.
      Same reason, same words, same fallback, so the two screens cannot say different things about
      one sale. The tenders on that sale are said in the shop's language too, with a wallet keeping
      the name the shop gave it. Both went into protocol version 3 rather than earning another bump,
      because nothing has shipped between them

- [x] And the receipt lookup keeps its older shape too. Bumping the protocol covers the queue and the
      receipt both, and the server promises to answer a client one release behind: a back office
      that asked for a receipt would have been handed a shape it could not read. Both branches are
      mutation tested, and both tests need two rows to bite, because with one the extra field lands
      at the end of the body where a decoder ignores it

- [x] A shop can say which of its goods are exempt when it brings its list in. Everything imported
      landed standard rated, because the file had no column for it: a shop selling puffed rice and
      exercise books would have declared tax on goods that carry none, and had to open every item
      afterwards to fix it. The column reads the words a shop writes, in either language, and the
      export writes them back so the round trip keeps the answer. A rate is no longer shown beside a
      row that is exempt, because the arithmetic charges nothing whatever rate the item carries and
      the two together read as a contradiction. Walked live: three rows written, and the shop's own
      form shows the puffed rice as ভ্যাটমুক্ত

- [x] And whether a price already has the tax in it, which matters more. A great many shelves here
      are priced at MRP: the number on the packet is what the customer pays, tax and all. Every
      imported price was read as tax exclusive, so the till would have added fifteen percent on top
      of a price that already carried it, on every line of every sale, until somebody opened eight
      hundred items and ticked a box. The column takes yes or no in either language and the export
      writes it back. Walked live: a packet imported at twenty taka MRP, and the shop's own form
      shows the box ticked

- [x] A refusal the server gives was the last English left. `ProtocolError` now carries a code,
      `refusalNamed` in the bindings gives the code, the figures already formatted and the English
      sentence together, the worker hangs them on the error it throws, and the bridge sends them as
      their own fields: an Error does not survive a postMessage with anything hung on it, which is
      the part that gets a test of its own. Two names are deliberately kept apart from the till's
      refusal of the same shape, because a till refusing "not permitted" is a cashier who may not and
      the server refusing it is a device that may not; the frozen lists fail if a name is ever in
      both. Broken deliberately to watch the guard fail, on the code and on the plumbing.
      Walked, once the browser came back: two items saved with one barcode, and the shop's refusal
      read on the screen in Bangla with the barcode as a figure inside the sentence
      ("আপনার বিক্রি করা আরেকটি পণ্যের বারকোড আগে থেকেই 9999000011112"), then the same refusal read
      in English after switching the language back

- [x] External review of the two commits above. Ten findings, nine fixed. The import gate could be
      locked out for good by a row with neither a code nor a barcode, because nothing about it could
      ever arrive to satisfy the wait; it recorded a row only after a successful save, so a reply
      lost on the way back let the retry add a duplicate; and it did not record a barcode appended to
      an item the shop already had, so the next file carrying only that barcode read as new. A
      corrupt reason in a backup restored silently as no reason at all, where every other field in
      that record refuses. Sixteen tooltips and placeholders were still English, including the only
      thing a screen reader has to go on, and the shared file reader answered in English prose that
      a Bangla back office showed as it stood. Two more scans now cover both: markup attributes, and
      anything a shared module hands back. Both broken deliberately and watched to fail

- [x] "Written, tested, shipped, reachable by nobody" was four separate defects this month, each
      found by walking weeks later, each looking finished in the commit that added it because the
      handler's own tests passed. It is a guard now rather than a habit of auditing. Three links, one
      per boundary: every route the server serves is posted to by the bindings, every request the
      bindings build is asked for by a screen, and every command a till can be given is run by one.
      The lists are written out of the enums themselves, because a list copied by hand goes stale the
      first time somebody adds a variant. Nine commands are reached by something that is not a screen
      and each now carries a reason beside its name: six through a named bridge method, one across
      the C ABI for an Android till that does not exist yet, and two that are the thermal path and
      genuinely reach nothing. The first version of the route scan matched `.route("` and silently
      missed the three routes the formatter had wrapped, which is a reachability test that has
      stopped checking reachability; it skips the whitespace now. Broken deliberately in both
      directions and watched to fail

- [x] The app can be opened with the internet down, which is the promise everything else here is
      built for and the one thing that did not work. Walked first to be sure: with the app's own
      server stopped, a reload showed a browser error page, and every offline thing underneath it
      (the ledger on the device, the catalogue replica, the offline sign-in, the log of what is
      unsent) might as well not have existed, because the browser could not fetch the page or the
      wasm to reach any of it. Each app now keeps a copy of itself, with the file list taken from
      the build rather than written by hand: a hand-written list is a list missing whatever the
      bundler renamed, and the shop finds out at the worst moment. The build refuses to write a copy
      with no wasm or no page in it. Walked end to end with both servers stopped and the tab
      reloaded: the till came back, signed a cashier in, rang two of something at 115.00 with the
      tax worked out, took cash and printed, and when the shop came back the sale drained and was
      accepted (`carried=1 accepted=1 quarantined=0`)

- [x] A new build never takes over mid-sale. A worker that swaps the running code the moment it has
      it is a screen that reloads under a cashier, and at worst a basket rung under one version of
      the pricing rules and finished under another. `skipWaiting` is deliberately not called on
      install; instead the app watches for a moment with no basket, no money on a ticket, nobody
      counting and nothing unsent, and only then lets the waiting build in. A till's tab is never
      closed, so the browser's own default would have meant waiting for ever

- [x] Every date box in the back office was filled in with the date in Greenwich. The range those
      boxes describe is read from local midnight to local midnight, so east of Greenwich the two
      disagree for the first hours of every morning and Bangladesh is six hours east: a shopkeeper in
      Dhaka opening the trail at five in the morning got a window that ended at midnight the night
      before, and was told nothing had happened. The day report was worse than empty, because it
      defaulted the same way and showed yesterday's takings under today's heading with nothing on
      the screen to say which day it was.

      Found by running down an empty trail list rather than by reading anything: the entries were
      there the whole time. The figures were never wrong, only the date the box started on, which is
      the part somebody reads. Seven places across both apps, one tested function, and a test that
      fails if `toISOString` comes back to either screen. Walked: the boxes now read 09/09 where the
      browser's own UTC date is still 08/09, and the trail came back with the drawer opening in it

- [x] And with the window right, the drawer opening is proved end to end: pressed on the till,
      allowed by the operator's own permission, pushed to the shop, and read back in the back office
      as "the drawer opened · 09/09/2026, 05:37:04 · Demo Owner · their own permission covered it"

- [x] A shopkeeper who has just fixed the line has something to press. The backoff doubles to five
      minutes, which is right for a device retrying on its own and wrong the moment a person is
      watching: they restart the router and the till says it will try again in four minutes, with
      nothing to do but wait for a wait that exists to protect a server they can see is up. "Try now"
      clears the wait without clearing the count, so pressing it during a real outage does not turn
      the backoff into a fixed one-second retry, which is the thing it exists to prevent. Shown only
      while rounds are failing, because a button offered when everything works is one somebody
      presses instead of trusting the loop.

      The first version read `round.ok`, which was wrong and only walking showed it: a round that
      decides to wait is `ok` too, and during a backoff most of them are, so the button appeared for
      two seconds and then vanished for four minutes. It reads the driver's own failure count now.
      Walked: "trying again in 48s", pressed, "catching up" and "reached the shop 06:15:17"

- [x] A receipt and the ledger agree about when a sale happened. The paper read the clock a second
      time, so a sale committed at 23:59:59.9 and printed a fifth of a second later put the
      customer's copy in a different day from the shop's books, which is the one disagreement a
      receipt exists to settle. One reading now, used for both

- [x] The till and the back office stopped deleting each other's offline copy. A browser's caches
      belong to the origin and not to a worker's scope, and both apps are served from one origin, so
      a copy named on the build alone meant each app's worker deleted the other's every time it took
      over a new build. The back office is the one that would have found out: it is opened once a
      week, by which time the till has replaced its build several times, and it is the app likeliest
      to be opened on the morning the line is down. Found by watching two caches sit on one origin
      during a walk, an hour after the copies were built and while looking at something else.
      Nothing has shipped under the old naming, so no cleanup for it was written: the stale entry
      that existed only on the walk machine was deleted by hand

- [x] A wallet tender was recorded with no name whenever the cashier accepted the default, which is
      every cashier. `walletName` started empty and the dropdown's `bind:value` matched no option, so
      the browser showed bKash and the binding held nothing: the sale went through as a wallet with
      no name, and the drawer report then reads "a wallet 2,400.00", which the comment beside that
      dropdown says in as many words it must never say. The binding is made to agree with what the
      screen is showing, and a wallet tender with no name is refused as well, because that is the
      check that survives somebody changing how the list is loaded. Walked: accepting the default
      now prints "bKash 57.50" where it printed "a wallet 57.50"

- [x] The amount for a wallet, a card or an account was typed into a box labelled "Cash taken". A
      cashier who read the label and did not type there was refused with "enter an amount in taka"
      and nothing to say where. One box still, because the row beside it takes cash and the row below
      takes the rest, but it says what it is for whichever is selected

- [x] A sale paid in cash with no change to give read "Cash 57.50 ·" in the back office: a separator
      promising something that is not there. Between the tenders now rather than after each

- [x] The refund path walked end to end and was right at every step: signs turned round on the
      screen, the drawer back from 2,057.50 to 2,000.00, paper headed REFUND and naming the receipt
      it reverses, its own number, the shop accepting it unquarantined, the original receipt reading
      "57.50 has been given back against it", and the goods back on the shelf at the figure they
      started from

- [x] The account path walked end to end and needed nothing, which is worth writing down as plainly
      as a defect would be. A customer written down with a limit; a sale on account naming them on
      the customer's own copy; the drawer correctly unmoved, because money on account is not money in
      the till; the receivables list showing 57.50 against one entry; a part payment of 20.00 leaving
      37.50 over two entries; and the khata page the customer takes home reading sale 57.50, paid
      -20.00, owing 37.50.

      The limit was then dropped below what they already owed and a further sale rung against it. It
      was allowed rather than refused, which is the design: the operator's own permission covered it,
      and the trail says so in the words that had none until this morning, "sold to somebody already
      past what they may owe · Demo Owner · their own permission covered it". A cashier without that
      permission is refused and prompted for a supervisor, which the core's own tests cover and this
      walk did not reach

- [x] The quarantine path walked end to end in Bangla, which is the work built this morning and
      until now only unit-tested. A refund typed against a receipt the shop has never issued is given
      back at the till, because the goods came back and the money went out and only a person can tell
      that from a till whose sales have not arrived yet. The shop held it (`accepted=0
      quarantined=1`), and the back office worded the reason in Bangla with the receipt number as a
      figure inside the sentence rather than baked into it: "এটি T0000-999999 রসিদের টাকা ফেরত দেয়,
      অথচ এখানে ওই নম্বরের কোনো বিক্রি নেই".

      Deciding it demanded a note first, in Bangla, and said why: somebody will read this in six
      months. Striking it out said it had come out of the takings, the VAT and the stock, and the
      shelf figure moved to prove it: a held sale is counted until it is decided, so the refund had
      put one back on the shelf and the strike-out took it off again. Reconciled against all seven
      sales this walk rang rather than taken on trust.

      Sales held before `sale.quarantine_kind` existed still show their stored English sentence, and
      two of those sat in the same queue reading "rung at 1500 and it arrived at 1788805006314".
      That is the documented fallback and not a live defect: today's wording says "3 days before"
      and has a test forbidding the raw number, which those rows predate

- [x] A receipt printed again is written down. A second copy is a second piece of paper somebody can
      hand over: an expense claimed twice, a return made against a sale already returned. Not
      permission-gated, and that is the decision rather than an omission, because a customer who lost
      their copy is the ordinary reason and a till that needed a supervisor for it is a till a shop
      works around. What a shop reads is the shape rather than the single event: one on a Tuesday is
      somebody who dropped their paper, six on a Thursday evening by one person is something else,
      so each is its own entry

- [x] A first pass at the till for somebody standing at a counter with a queue. The name and the
      status ran together at the top and read "openposon this device"; the status was six things in
      one line competing with the money, and is now a quiet second line, because a cashier reads it
      perhaps twice a day and reads the total on every sale. The total is the number said out loud
      and leaned over the counter to read, and it was the same size as the word beside it: it is
      twice that now, and the change to hand back with it. Buttons are sized for a thumb on a cheap
      tablet rather than a mouse on a desk. The scan box, where a cashier's cursor lives all day,
      looks like the field it is. The drawer and the cash movements are set apart, because selling
      and counting were stacked at the same weight and the selling screen ran straight on into the
      float

- [x] Nine more pieces of English were sitting in the markup as plain text, where neither the scan
      for a sentence assigned to a message slot nor the one for an attribute could see them: "Back to
      scanning", "Take them in", "That is not a bundle. Check the whole of it was copied.", the whole
      counted-drawer line, "no longer sold". Two more were built in a function and returned, which is
      a fourth hiding place: the shelf warning a cashier reads with a customer in front of them
      ("the shop has -9, this wants 1") and every wording of what came off a line as a discount.
      Both scans exist now, and the one for returned sentences covers the screens as well as the
      shared files. Walked in Bangla: the shelf warning reads "দোকানে আছে -9, এখানে চাওয়া হচ্ছে 1"

- [x] The back office has a way down it. Nine screenfuls and twenty-two sections with nothing but
      scrolling: a shopkeeper who wanted to see who owed them money went past thirteen things they
      were not looking for. A sticky bar of jumps now, named by the headings themselves so they are
      already in the shop's language and cannot say something a section does not, and read off the
      page rather than written out, because a hand-written list of sections goes stale the first time
      somebody adds one and the symptom is a menu that quietly stops mentioning something the shop
      can do. Walked: five thousand pixels to "Who owes you", and the heading lands clear of the bar
      that took you there.

      Two mistakes on the way, both worth keeping: a `$effect` that read the state it writes never
      ran a second time, and `void enrolled;` as a way of declaring a dependency is a statement a
      minifier is entitled to delete, and did. Neither showed up as an error; both looked like a
      feature that simply was not there

- [x] Back office buttons stopped breaking their own labels in two. "Took payment" and "What is
      this" each read as two stacked words in a list somebody scans down. And a text box in a tight
      row could squeeze to about twenty pixels: the one beside "Strike off", where a shop writes why
      it is striking a debt off, showed none of its own placeholder. An unlabelled empty box is a box
      nobody fills in, and that note is the whole record of the decision

- [x] Two things found by using the till rather than reading it. A basket line is the button that
      opens the quantity stepper and nothing said so, so a cashier who wants three of something has
      to guess that tapping the line is how: it could not be found while walking, by somebody who had
      read the code. There is a mark on the row now, faint because it is the only thing there that is
      not a fact about the sale, and it turns when the row opens. And the box beside "Park it" was
      labelled "Whose is it?", which reads as the customer on the sale and was mistaken for exactly
      that: it is the name a parked basket is found again by, a table number or the man in the blue
      shirt, and it says so now

- [x] Both screens hold together on the device this is for. The product is aimed at cheap tablets
      and had only ever been looked at in a desktop window. The till was fine at four hundred pixels
      with nothing off the edge. The back office was not: rows of a text box and two buttons could
      not wrap, so they spilled sideways and what a shopkeeper needed was past the edge of the screen
      with nothing to say it was there. Rows wrap now, in both apps, and so do the controls on a
      list row. The tills row needed more than that, because three columns cannot share a narrow
      line: letting the name shrink squeezed it to three pixels, which is worse than scrolling, so
      below a small tablet's width the two buttons go under the name and take half the row each,
      which is also a bigger thing to hit with a thumb. Measured rather than eyeballed: ten elements
      overflowing before, none after, and the wide layout unchanged

- [x] A message a person cannot see is a message that did not happen. Both apps rendered what they
      wanted to say at the top of the page, and the back office is nine screenfuls: a shopkeeper who
      pressed "Strike off" at the bottom of it was answered five thousand two hundred pixels above
      the fold. Nothing appeared to happen, so the thing to do was press again, on a button that
      takes a debt off somebody's account. Measured before and after: 5,236 pixels above the screen,
      then twelve pixels below the top of it. Fixed to the screen rather than to the page, in both
      apps, and never on paper

- [x] Finished the narrow-screen work, and undid a regression I had caused with it. Letting rows
      wrap fixed small screens and broke every screen: the boxes in those rows are `width: 100%`,
      which is right when a box is alone and wrong beside a button, so once the row could wrap the
      button dropped below its own field at every width. Found by looking at the screen after the
      narrow fix, which is the only way it would have been found: nothing failed and no test could
      see it. Boxes share the line with their button now and give way only when there is genuinely
      no room. The tills row needed one more pass, because its buttons say things like "Code for this
      back office" and do not wrap: two to a line needed more width than a small phone has, so on
      small screens they take a row each, which is also the biggest a thumb can be given.

      Measured at 352, 400 and 512 pixels, in both apps: nothing overflowing at any of them, and the
      wide layout unchanged with the name and its buttons still sharing a line. What is still not
      proved is a real device: the browser here refuses to resize its own window, so the narrow
      layout is measured by constraining the page and by forcing the rules that a narrow viewport
      would turn on

- [x] Parking a basket walked end to end, including across an outage, and needed nothing. A ticket
      parked under a name; the next customer served while it waited; the parked one brought back
      whole and sold under its own number. Then the part that matters: a basket parked, both servers
      stopped, the tablet reloaded, and the app came back from its own copy with the parked basket
      still in the list. The cashier signed in with nothing reachable, brought it back, sold it, and
      when the shop returned the sale drained and was accepted unquarantined.

      That is the offline shell built this morning, the standing state that holds parked tickets, and
      the park feature, all working as one thing. Worth writing down as plainly as a defect would be:
      no defects

- [x] Two things a drawer count showed, both about naming money. The screen listed a sale on account
      as `till.credit`, the dictionary key itself, on the report a shop counts its takings against:
      `till.cash` and `till.card` existed and the third did not. Nothing could have caught it, and
      that was my own doing: those three keys are built at run time from what the core calls a
      tender, so the scan for keys a screen asks for cannot see them, and the scan for words nothing
      asks for explicitly skips them. Both holes, in the same three keys. They are checked directly
      now, against the set the core actually produces.

      Worse on paper. The drawer slip printed `format!("{:?}", kind)`, so a shop's own record of its
      money carried `Wallet("bKash")` and `Credit` while the screen beside it said `bKash` and
      `On account`. A debug representation is for whoever is reading a log; that slip is a document a
      shop keeps. There is one way of naming a kind of money now, shared with the receipt, because
      two ways is how the paper and the screen came to disagree. The test fixture had only cash and a
      card in it, which is why this survived: it has a wallet and an account sale now, which is what
      most shops this is for actually take. Walked: "bKash (not in the till)" on a real slip

- [x] The rest of the drawer count was right. X report mid-shift and Z report at close, the opening
      float, each kind of money separately with the ones that never reached the drawer saying so,
      and the arithmetic: 2,000 float plus 317.50 cash is 2,317.50 to hold, counted 2,277.50, out by
      40.00

- [x] Paper prints in English, whatever the screen is set to. Three reasons pointing one way: no
      ESC/POS code page carries Bangla so a thermal printer gets English regardless, the layout pads
      by counting characters which Bangla defeats so a Bangla slip comes out ragged, and a shop with
      two languages on its counter should not keep two shapes of receipt in its records. Walked: a
      Bangla till printing an English receipt. `paperWords` has no caller now; the mechanism and the
      words stay, because they are what makes paper translatable at all and the raster path will want
      them, and they are still checked to exist in both languages so they cannot rot while they wait

- [x] The jump bar was missing exactly the sections worth jumping to: twenty-three on the page and
      twenty ways down to them, and the three absent were the ones that only exist when a shop has
      something to look at, sales needing somebody, items a till wrote down, prices that never
      reached a till. Two of my own bugs, one behind the other. The ids were numbered by position, so
      a section appearing later took a number one already held, and a keyed list with a repeated key
      silently renders fewer things: no error, nothing in the console, just three missing. They are
      named for their headings now. Behind that, the watch on the page was never installed, because
      it looked the element up instead of binding it and got null

- [x] The offline copy could be built out of the last build's files. `cache.addAll` goes through the
      browser's own HTTP cache, so a device that had installed a new build held the new script and
      the old page, and the old page named the old script: it went on running the previous build.
      Every symptom of that looks like a change that did not work, and it cost an hour of chasing a
      fix that was already correct before the build itself was looked at. The copy is taken from the
      network now

- [x] The build refuses a copy whose page names a file it would not hold. Directly out of the
      staleness bug above: the bundler renames its assets every build, and a copy holding a page that
      points at a script it does not have is a device that boots to a blank screen the first time it
      is opened without a line. Nothing else would notice, because the app works wherever it can
      reach its server. Broken deliberately and watched to refuse

- [x] The stock take walked end to end, barrier and all, and needed nothing. An item at minus
      fourteen on the books, counted at forty on the shelf: the figure became forty rather than
      twenty-six, which is the whole point of a count being an assertion rather than a movement. The
      sheet shows what the books say beside each box, so whoever is counting can see the difference
      as they write it. Then a sale after the count took it to thirty-nine, so the barrier is a
      starting point and not a freeze

- [x] Carrying sales in by hand walked end to end: the last resort for a device the shop will not
      take sales from. Revoked a till holding one sale, and it said so plainly rather than looking
      broken; read the sales off it with a mark and a length to check against; carried the text
      across; the back office worked out the same mark independently, 4e85 cdbb, before taking
      anything in.

      One defect at the end of it. The server is careful: a sale the shop already had is taken in and
      joins no queue, with a comment saying that flagging it "would send somebody looking for a queue
      entry that is not there". The screen then said "Taken in 1 sale(s). They are in the list below
      for you to check" whatever happened, and its own explanation promised every carried sale joins
      that list. So the screen undid the server's care one layer up, and I walked straight into it:
      hunted through the queue for an entry that was never going to be there. The count of what is
      actually waiting now crosses the boundary, and the two cases say different things

- [x] The purchasing side walked: a supplier written down, a delivery booked against them with a
      challan number and a cost each, the goods on the shelf, the money owed, and a part payment.
      The arithmetic held throughout: twenty at thirty is six hundred owed, two hundred and fifty
      paid leaves three hundred and fifty, another five at thirty makes five hundred across two
      deliveries. The shelf reconciled against every sale rung in between. Deliveries are filed under
      the supplier with their challan number, so goods and invoice can be put side by side, and
      nothing is stored as a balance: what a shop argues about is the deliveries, and they are
      listed.

      One defect. What the shop owed did not refresh when a delivery was booked, so a shopkeeper
      booking six hundred taka of goods from a named supplier could look down the page and read "you
      owe your suppliers nothing". The figure was right and only a reload showed it, which means the
      reasonable conclusion is that the delivery lost the supplier. Walked, and that is exactly what
      it looked like

- [x] A separator swallowed its own space: a supplier's phone ran into the BIN after it,
      "01911223344· BIN". The space sat inside the `{#if}` that follows it and was eaten at the
      boundary. Two places, both fixed

- [x] The whole recovery cycle for a cut-off device walked, and needed nothing. Revoked, carried its
      sales out by hand, then put back into service. The part worth naming is what it refused: a code
      for a *different* till was rejected while this one still held a sale nobody had taken, because
      enrolling as another terminal leaves that ledger where nothing will open it again, and those
      are sales that happened. It costs a spent code to find that out, which the code says out loud
      is the cheaper of the two things to lose. The right code, for that same till, put it straight
      back: not refused, nothing waiting, reached the shop. The back office's own row said what to do
      in the meantime, "holds nothing: it needs a code"

- [x] The day report walked, and the whole chain from a delivery's cost to a day's margin with it.
      A cost of thirty booked in against a supplier, pulled by the till, frozen onto the next sale,
      and the report then reads "Made 20.00 on 50.00 of selling before tax, against 30.00 the goods
      cost you. Over 1 sale(s)." One sale, because only one carried a cost. The other nineteen are
      named rather than averaged away: "19 sale(s) of 875.00 are not in that figure: something on
      them has no cost written down." A report that refuses to claim a margin it cannot support is
      the right kind of report.

      Takings moved 1,006.25 to 1,063.75 for a 57.50 sale, refunds are counted in and said to be
      counted in, and a counted drawer stays as it was counted even when a sale in it is struck out
      afterwards, which the screen explains rather than leaving somebody to find. Nothing to fix

- [x] Backup and restore walked as an operator, not as a test: the path a shop only ever uses on its
      worst day. Exported the whole shop to a file, a hundred and forty lines with a trailer counting
      what should be in it. Verified it without a database and without writing anything, which said
      "this bundle reads whole" and the counts. Cut it off at line a hundred and it refused with
      Truncated; corrupted one field and it refused with the line number. Restored it into a separate
      shop, and the two now hold the same twenty-four sales and the same 2,489.75 to the poisha.

      Two things worth naming. The restore warns that no PIN travels in a bundle and names how many
      people need one before anybody can sign in, which is the sort of thing a shop otherwise
      discovers at a counter. And the quarantine reasons came through: four held sales in each, one
      carrying its reason as bytes and three that predate the column. That is this morning's fix
      proved through the operator path rather than through its own test.

      The restored copy is left in the dev database as shop `…00ff`. It collides with nothing, and
      deleting a tenant is not something to improvise

- [x] Which receipt was reprinted is recorded. A change to the trail's shape on disk rather than to
      a wire, which is the one place where a mistake costs a shop the day's unsent sales and its
      parked baskets rather than a worse error message, so it was done on its own: `AllowedV1Legacy`
      frozen as the entry stood, the seven older standing states repointed at it, `TERMINAL_SCHEMA`
      to 15 with a `TerminalStateV14Legacy` beside it, and `AllowedV1` added to the list of shapes a
      legacy struct may not name. Version 14's bytes are frozen in `bytes_from_before.rs` carrying a
      reprint still owed to the shop: it reads back with no receipt number, because the device that
      wrote it did not know one and nothing may be filled in for it afterwards. Freezing 14 needed a
      `CustomerV4Legacy` too, since the cap on what somebody may owe arrived in that version and the
      state named the live customer.

      Which receipt is answered by the bindings from the page that was laid out, not by the screen.
      The same button prints a drawer slip, a customer's account and a sale, and a screen asked for a
      receipt number would answer out of whatever it was holding; that answer lands in the trail a
      shop reads to decide whether somebody took money. So `Command::Reprinted` carries only the
      clock, and a reprint of a drawer slip names no receipt. Stored in Postgres as a nullable
      column, carried in a backup, and shown on the back office trail beside who and when.

      Walked, and the walk found the defect: the till wrote the number, the wire carried it, the
      shop's store held it, and the crossing into the screen dropped it. `sync::Allowed` is the
      shape the back office reads and it had no field for it, so the trail said "printed a receipt
      again" with the number sitting in Postgres two feet away. The same shape as four earlier
      defects this month: a lower layer being careful and the last hop throwing the care away. A
      bindings test now decodes a trail reply carrying a reprint with a number, one without, and a
      drawer opening, and was watched to fail with the mapping put back. The English underneath the
      dictionary also did not know actions 12, 13 or 14 and called them "something this build does
      not know about"; it names them now.

      A second miss in the same change: the screen edit anchored on markup that appears in two
      sections and landed in the waived-overrides list under What sold, where `receipt_no` is never
      set, so it rendered nothing and looked done. Found the same way. Anchor on text only one
      section has, and read the built bundle rather than the source when a change appears not to
      take. Walked end to end afterwards: a sale rung to T9527-000022, printed again, and the back
      office trail reads "printed a receipt again · Demo Owner · receipt T9527-000022"; then the
      drawer slip printed again, which names no receipt and does not borrow the sale's.

      Codex caught what the walk could not: the trail travels on two wires, and both changed shape
      without the protocol version moving. postcard is positional, so a till on the release before
      this one is not looking at a missing field, it is looking at a decode failure: its pushes
      would fail on a timer while it holds the only record of who allowed what, and a back office a
      release behind would find an error where the trail should be, on the screen a shop opens when
      it suspects something. Protocol 5, with `AllowedWireV4` and `AllowedEntryV4` frozen beside the
      current shapes and an arm at each end, which is what the repair queue already does for
      versions 1 and 2. Both arms were broken deliberately and watched to fail: without the first an
      old till gets a 400, without the second an old back office cannot decode the reply

- [x] The trail told a shop the opposite of what happened. An entry for somebody who tried something
      and was stopped, a line taken off a basket already paid towards or the drawer opened, rendered
      as "their own permission covered it": the screen a shop reads when it is investigating a till
      said the person had been entitled to do the thing they had just been refused. Only two of the
      four kinds of entry were being told apart, and the comment beside that field says in as many
      words that saying "on their own permission" about the others is telling the shop a lie. Four
      now: a wrong PIN, somebody taking the till, somebody stopped, and somebody permitted. A reprint
      is in a fifth position, needing no permission at all, because saying one covered it invites a
      shop to go looking for a permission to take away that does not exist. Walked as a cashier:
      "tried to take a line off a basket that had been paid towards · Rina · and was not permitted to"

- [x] A role is one thing now, and it is decided in the core. It was decided twice and the two
      disagreed: the core's cashier could not open the drawer and the shop's could, the shop's
      supervisor was capped at a fifth off and the core's at everything. A shop picks one of two
      from a dropdown and the screen sent what the choice meant, so the choice was a rule living in
      a screen, which is a rule the Android till does not have. `EVERY_ROLE` and
      `Permissions::named` in the core, exposed through the bindings, asked for as the back office
      boots, and saving refused until the answer arrives: sending a person with no permissions field
      is a request the core will not decode, and the shop would be told the save failed on a screen
      where it had every reason to work. A scan of both screens fails on anybody setting a
      permission flag, and it was watched to fail with the old copy put back.

      Walked: a cashier and a supervisor added through the back office arrive in Postgres with
      exactly what `core/src/auth.rs` says, having crossed the wasm, the worker, the screen and the
      server. They are still in the dev demo shop as `Walk Roles Cashier` and `Walk Roles
      Supervisor`; deleting rows behind the app's back is not something to improvise.

      Codex then found the second half of the same defect, which had been there since the cart was
      written: an allowance lifted the basket's ceiling to everything. A supervisor approving fifteen
      percent left the cashier able to give ninety on that ticket without asking anybody, while the
      trail said "allowed a discount of 1500 basis points". A record that describes something that
      did not happen is worse than no record, because it clears somebody. The ceiling now rises to
      what was allowed and no further, a ceiling already higher is left alone, and a price typed
      over the catalogue's opens that door without touching the discount ceiling. Broken deliberately
      and watched to fail. Every test until now asked for one discount and stopped, which is why
      nothing caught it

- [x] The cash drawer opens. `escpos` laid out a receipt and never sent the pulse, so a shop on a
      thermal printer opened the drawer by hand two hundred times a day, and the till's button
      called "Open drawer" started a shift, which is a different act with a nearly identical name.
      Five bytes, and every one matters: pin 2 because that is the standard wiring, and fifty
      milliseconds on rather than longer because a pulse held cooks the coil in a cheap drawer. Its
      own job rather than a flag on a receipt, since a cashier giving change for something bought
      next door prints nothing and a receipt that always kicked would open the drawer on a reprint.
      Gated on the same permission a cash movement is, because it is the same act: the drawer coming
      open with nothing on the paper to say why. Written into the trail either way, and a refusal
      under its own number: eleven means a line taken off a paid basket, and saying that about
      somebody who tried the drawer would accuse them of something else.
      Walked as far as the screen: the relabelled button reads "Start the drawer", a drawer opened
      with a 2,000 float, "Open drawer" appeared as its own button beside the cash movements, and
      pressing it was accepted with no refusal and left the drawer figure alone. No printer has ever
      been near these bytes

- [x] Action twelve had no words, so a Bangla shop read English for it from the day it was added: a
      sale to somebody already past what they may owe. Nothing could have caught it, because trail
      entries are asked for by number rather than by name and the test that scans screens for keys
      cannot see them. The numbers a till can write are frozen now, read out of `till.rs` itself
      rather than listed twice, and handed to the JavaScript the way the refusal codes already are.
      Numbers are never reused: a shop's stored trail is read under that list, so one that changed
      meaning would be last year's evenings quietly saying something else

- [x] `NotAPrice` was the last refusal carrying an English clause where every other one carries a
      figure, so a Bangla shop read its own sentence with English inside it. It is three refusals
      now, each with its own figure: a rate that is not a rate, a price below nothing, a cost below
      nothing. Appended at ten, eleven and twelve rather than replacing nine, because these encode
      positionally and an older till must not read one refusal as another; `NotAPrice` stays where it
      is and a caller still speaking 3 is handed it, with the sentence it has always had, because a
      build from then cannot decode the new shapes at all and would fall back to a bare status
      number. Protocol 4. Both shapes have a test

- [ ] A Bangla paper reads ragged on a screen. The papers are padded by counting characters so an
      amount lands in the same column on a fixed-width printer, and Bangla defeats that twice: a
      matra draws no column of its own, and almost no machine has a monospace Bangla font, so the
      browser draws proportionally whatever the count says. Counting columns rather than characters
      was tried and reverted, because it makes the count right and the screen no better: the width is
      the font's, not the string's. The real answers are a screen that lays the receipt out itself,
      which is the second implementation `receipt::Line` exists to prevent, or the raster path Bangla
      on thermal paper needs anyway. On paper today the printer prints English and the columns are
      right The receipt's words are built in the core, and
      the way to do it is the way the refusals went: the caller supplies the words and the core holds
      none, so the ESC/POS path keeps English (thermal paper cannot render Bangla at all) while a
      browser-printed one can be in either Digits stay Western, which is what
      most Bangladeshi shops use on a screen, and the Bangla has not been read by a native speaker.
      Both are worth settling before a shop sees it

- [x] External review of the import, the deletion guard and the price check. Nine defects, all now
      fixed. The two worth naming: a sale's quarantine reason was written to the shop's table and
      dropped by backup and restore, so a shop that restored from a backup got its held sales back
      with the prose and nothing a Bangla screen could word; and an item withdrawn rather than
      deleted was only checked against stock movements, so an item with a stock count, a delivery
      line or a correction against it and no movement was still deleted out from under them

- [x] Five sentences survived the translation by skipping the dictionary altogether, assigned
      straight to the line somebody reads: two name clashes, a stale-copy refusal, the count at the
      end of an import, and what a till says after writing its carried sales to a file. Neither
      existing test could see them, because both test keys rather than what is said. A third now
      scans the screens for a sentence assigned to a message slot: two English words in a row and it
      names the file and the line. Broken deliberately and watched to fail before it was kept

- [x] Reading the same file twice in a row added everything twice. The rows just written live on the
      shop's server, not yet in this device's copy of the catalogue, and the copy is what the file is
      matched against; a run that refused half the rows is exactly when somebody fixes the file and
      reads it again. Walked, and it did exactly that: both rows read as new a second time.
      Two wrong answers before the right one, both found by walking rather than by reading. Marking
      the device as behind and waiting for a pull loses a race, because a pull already in flight when
      the write lands answers yes: it does have everything it asked for, and it asked before the rows
      existed. Matching the written rows by id instead left the back office refusing every import
      from then on, since an id is minted in the browser as a string, travels as a number and comes
      back written the shop's way. It now asks by code and barcode, which is what the matching itself
      uses. Walked: refused while behind, cleared itself when the pull landed, and the same file then
      read as two corrections

- [x] Two windows on one device turned a working till into one that looked wiped. OPFS gives the
      files to whoever asks first, so the second window got the browser's own sentence about access
      handles, in English, at the top of the screen, and underneath it the box asking for an
      enrolment code. Nothing was wrong: the ledger was open a swipe away. A shopkeeper who followed
      that screen would have enrolled the device again, minting a second terminal with its own block
      of receipt numbers while the sales, the parked baskets and the numbers already handed out
      stayed in the window nobody was looking at. The screen invited the one move that loses
      something. Found by opening two till tabs while walking something else.

      The reason is named once, in `apps/shared/storage_trouble.js`, as a plain function over
      whatever the browser threw: open elsewhere, no room, or a browser that keeps nothing, which
      are three different things for a shop to do. Nothing there touches a browser, so the failures
      are written down as Chrome and Safari produce them and tested. The screens say it from the
      dictionary and hide the enrolment box for that one reason only, offering "try again" instead,
      which is the whole answer once the other window is closed. The back office keeps a store the
      same way and is the likelier of the two to be opened twice, so it got the same treatment.

      One trap on the way: what the browser throws is a `DOMException`, whose `code` is a read-only
      getter from an older standard. Hanging the reason on it inside a module throws a TypeError, so
      the name meant for the screen would have become a second failure thrown from the handler for
      the first. The failure is carried in a new Error with the original underneath it, and there is
      a test that models a read-only `code` and would fail if that went back.

      Codex found three gaps in the first cut. The guard covered only the last call in opening a
      store, so a failure part way through the list left this tab holding files it had no record of,
      and a browser refusing storage outright still arrived as its own sentence: the whole of
      opening is under one guard now. Enrolling opens a ledger too and could meet the same lock,
      which left the box on the screen telling somebody to do the thing that loses their sales.
      And the test that says every code has words scanned the file as text, so a code that appeared
      only in a comment would have passed: it asks the dictionary now, and checks Bangla is not
      English.

      Walked: two till tabs, then two back office tabs, then the reviewed build in Bangla. The
      second window says it plainly in the shop's own language with no enrolment box, and closing
      the first and pressing "try again" opens the ledger with its 477 receipt numbers and resumes
      syncing

- [x] A message already on the screen follows the language now, like the buttons around it. It used
      to hold a sentence, worded at the moment something went wrong, so switching to Bangla re-worded
      every label and left the one line the person was reading in English. Seen while walking the
      two-windows message: the buttons turned over and the sentence did not.

      Deferred a dozen times as 69 assignments and 65 call sites. It was neither: `t` now hands back
      something that holds the key and the figures and says itself when it is read, so the change is
      `t` and `refusal` themselves, and every site that already called them followed. `say` turns
      whatever fills a brace into text, so a refusal folded inside another message is worded late as
      well, which is the import panel's "line 4: ..." and was the one hard case.

      Two things fell out of it that no scan could have found, because both were code rather than
      prose. The till coloured its sync line by looking for the words "held up" in the sentence: on a
      Bangla till the words are not there, so the colour that says a till has stopped reaching its
      shop only ever appeared in English. And two states were written into the screen as the code's
      own word: "idle" before the first round, and the ledger's state where no ledger could be
      opened, which said "unavailable" in English on a Bangla till in exactly the moment a
      shopkeeper needs to read it. Both now come from the dictionary, the second by its code with
      the code itself as the fallback.

      Walked live on a Bangla till: a discount over the ceiling refused, the panel and the figures
      read in Bangla ("ছাড় চাওয়া হয়েছে 90 শতাংশ, আপনি দিতে পারেন 0"), the language switched under
      the refusal, and the same sentence read in English with the same figures, without the refused
      work being run again. The header's "starting up" and "nowhere to keep this" read in Bangla too

- [x] Five things a shop reads were still in English, and the scans that exist to catch exactly that
      read straight past them. A sentence behind a ternary was invisible because the scan looked at
      the character after the `=`: "say how much to strike off" and "say how much they handed over"
      on the account screen, "some boxes do not hold a number yet" and "nothing counted yet" on the
      count sheet. Two more went in as the second argument of `attempt`, which nothing scanned at
      all: "Enrolled." and "Written down." And a button said "Look" in the middle of a page that
      had otherwise turned over into Bangla, because the markup scan wanted two words in a row and
      most of what somebody presses is one.

      The scans now read the whole statement rather than the character after the sign, skipping
      comments so an apostrophe in one does not read as a string, and only at the statement's own
      depth so a request field like `what: 'amend_operator'` is not mistaken for something somebody
      reads. One word between tags is enough to fail, with the shop's own name allowed. Broken
      deliberately and watched to fail.

      Walked in Bangla: every button on the back office now reads in Bangla except the one that
      names the other language, and pressing "book the count" with nothing counted says
      "এখনও কিছু গোনা হয়নি" where it used to say it in English

- [x] A cashier could not ask for a discount at all, so the whole supervisor path was unreachable
      from the till. The boxes were shown only to somebody whose own ceiling was above zero, and
      every cashier in every shop has a ceiling of zero: the preset says nothing unaided. So the
      customer asked for ten percent off, the cashier had nowhere to type it, and the supervisor's
      PIN could not be offered because nothing had been refused. The way round it was for the
      supervisor to sign in and ring the sale themselves, which puts it under their name and is the
      workaround the trail exists to make unnecessary. Every part of the machinery was already
      built: the refusal names the rate, the screen offers the supervisors by name, the allowance
      goes onto the paper and into the trail. Nothing reached it.

      The boxes are offered to whoever is at the till now, and the placeholder says which case it
      is: "up to 5%" for somebody with a ceiling, "a supervisor allows it" for somebody without.
      Found by walking as a cashier rather than as the owner, which is the account every walk before
      this one used.

      Walked in Bangla end to end: the cashier asks for fifteen percent, is told "ছাড় চাওয়া হয়েছে
      15 শতাংশ, আপনি দিতে পারেন 0", the supervisor's PIN allows it, the basket drops to 171.06, and
      ninety percent afterwards is refused with "আপনি দিতে পারেন 15", which is the ceiling shipped
      earlier today doing its job on a real screen

- [x] The line that says who allowed what was cut in half on the paper. On a 58mm roll it read
      "Walk Roles Supervisor allowed a" and stopped, losing the part that says what was allowed, on
      the one line that explains why the price differs from the shelf. It is broken over as many
      lines as it takes now, on spaces, with a word longer than the roll cut because there is
      nothing else to do with it. And the rate is printed as a rate: the paper said "1500 basis
      points" where the shop asked for fifteen percent, which is a receipt written for the people
      who wrote the till. Found on a real receipt during the walk above, not by a test, because
      every test used a name short enough to fit

- [x] A parked basket lost what a supervisor had allowed on it, and could lose the basket. Review
      found both, on a path the change above had just opened to every cashier. Resuming took the
      ticket off the parked list and persisted that, and only then applied the ticket discount
      through the checked setter, which refuses anything above the ceiling of whoever is at the till
      now: a basket approved at fifteen percent, parked, and resumed by a cashier whose own ceiling
      is nothing was removed from the list and then refused, so the customer's basket was in nobody's
      hands. The cart is built first now and the list is touched only when it is built, and the
      discount and the waiver are put back rather than re-applied, the same way the lines already
      were: this basket was priced and approved before it was parked, and the person who resumed it
      is not the person who could approve it again.

      A parked refund came back as a sale with negative lines on it, because nothing written down
      said which way round it was, and the screen has offered "park it" during a refund since
      refunds existed. That is money going the wrong way with nothing on the screen to say so.

      Both needed the parked basket to carry more than it did, which is `TERMINAL_SCHEMA` 15 to 16
      with the whole careful dance: `HeldTicketV5Legacy` frozen as it stood, `TerminalStateV15Legacy`
      beside it pointing at frozen copies of the customer, the item and the trail entry, a decode
      arm, and version 15's bytes frozen in `bytes_from_before.rs` carrying a parked crate that comes
      back as a sale with no waiver, which is the truth about what that build knew. Both defects were
      broken deliberately and watched to fail

- [x] The price box was the same defect one control along, and it was still there after the
      discount boxes were fixed: shown only to somebody already permitted to type a price over the
      catalogue's, which a cashier is not, so the supervisor's PIN could never be asked for. It also
      carried what the line is priced at, so hiding the box hid the price. Offered to whoever is at
      the till now, with an aria-label saying which case it is, since it has a value rather than a
      placeholder and a screen reader had nothing at all.

      Walked as a cashier in Bangla: 175.00 typed down to 150.00 is refused with "এই ব্যক্তি দোকানের
      দামের বদলে নিজে দাম লিখতে পারেন না", the supervisor's PIN allows it, the line reprices to
      150.00 and the basket to 172.50, the receipt carries "Walk Roles Supervisor allowed a price to
      be typed over the catalogue's" across three lines, and the shop's trail holds it as action 2
      with the cashier's name and the supervisor's beside it. The same walk put a discount, a refund
      and a sign-in in the trail: four kinds of entry a cashier could not reach at all this morning

- [x] Closing the drawer refused a cashier and offered nobody, and three other commands did the
      same. The core answers a refusal with the action a supervisor would have to allow, the view
      carries it, and the screen has a panel that offers the supervisors by name: all of that
      worked, and whether a shopkeeper ever saw it came down to which helper the screen happened to
      call. `attemptWithOverride` keeps the refused work and offers the panel; `attempt` shows the
      sentence and drops it. Closing the drawer went through the plain one, which is the last thing
      a cashier does at the end of a shift: the count was retyped by a supervisor who had to sign
      in, so the shop's record of who counted the drawer named the supervisor. Opening the drawer,
      moving cash and the scan that follows writing an item down at the till were the same.

      All four go through the override path now, and a scan of the screen fails on any of the
      thirteen commands the core can refuse on a permission being asked for through the plain one.
      Broken deliberately and watched to fail.

      Walked as a cashier in Bangla: 2,000 float in, counted 2,000, refused with "সুপারভাইজার ছাড়া
      এই কাজটি করা যাবে না", the supervisor's PIN allows it on the spot, the Z report comes out
      "ঠিক মিলেছে", and the shop holds `closed_by_name = Walk Roles Cashier` with the trail saying
      the supervisor allowed it. The person who counted is the person the record names

- [x] A sale past somebody's credit cap said so in the trail and not on the paper. Three of the four
      things a supervisor can allow on a ticket went onto the customer's copy and the shop's, and
      this one did not, so the customer walked out with a receipt saying nothing about the only
      unusual thing about the sale: that the shop let them past a cap it had set itself. It is on
      the paper now, in the same place as the rest.

      Written down once, not twice. Unlike the other three it is checked through the auth book when
      the money goes onto the ticket, which writes its own entry, so an entry at the PIN as well
      would have a shop counting one waiver as two. The test asserts the whole trail rather than a
      count of one kind, because the first version of it passed while the second entry was quietly
      filed under "a price typed over the catalogue's".

      Walked as a cashier in Bangla, against a customer already 296.25 into a 50.00 cap: refused
      with what they owe, what they may owe and what this would make it; allowed by the supervisor's
      PIN; the receipt carries "Walk Roles Supervisor allowed a sale past what this customer may
      owe"; and the shop holds one entry, action 12, with both names on it

- [x] Every permission-gated thing a cashier can reach was walked as a cashier, which is how the
      three defects above were found. Eight of them: a discount, a price typed over the catalogue's,
      a refund, opening the drawer, moving cash, closing the drawer, a sale past somebody's credit
      cap, and a basket past the shelf. Each is refused with the figures in it, each offers the
      supervisors by name, each is allowed by a PIN without signing anybody out, and each lands in
      the shop's trail under its own number with both names on it. The last two were walked with the
      shop's stock rule turned up to "stop until a supervisor allows" and turned back down
      afterwards: the refusal reads "দোকানে Walk Two Atta 2kg আছে 4, আর এই ঝুড়িতে চাওয়া হচ্ছে 400",
      the paper says the supervisor allowed more to be sold than the shop has, and the trail holds
      action 10.

      Nothing new was found in the last two, which is worth saying: the shelf and the cap were
      already asked for through the path that fetches a supervisor, so they were right before today

- [x] A shop that changes a rule was told to wait up to ten minutes for its tills to obey it. It was
      never ten minutes: the shop's settings number is checked every thirty seconds, and a number
      that has moved makes the shop, the people and the account customers all due on the next round,
      which is two seconds later. The ten-minute figure is the cadence those three lists are re-read
      on when nothing has changed, and it is what the messages were written against before the
      settings number existed. Five of them still said it: shop details saved, a new PIN, somebody
      let back in, a name corrected, somebody suspended.

      Measured rather than reasoned about. The stock rule was saved in the back office at 10:12:18
      and the till at the counter refused a scan under the new rule at 10:12:27: nine seconds. All
      five now say half a minute, in both languages, which is the bound the cadence gives rather
      than the number I happened to measure.

      Twenty times wrong in the shop's favour is still wrong, and it is the kind of wrong that
      teaches an owner not to believe the screen: suspend somebody, read "ten minutes", walk away.
      `words.test.js` now reads `IDLE_MS` out of the driver and fails if the cadence moves past what
      those phrases promise. Broken deliberately, at two minutes, and watched to fail

- [x] Seven of this shop's catalogue rows had stopped decoding, so every till was selling those
      items at whatever price it already held and the back office was telling the owner to type the
      prices in again. `ItemWire` gained `from_a_till`, then `supply`, then `category`, each
      appended correctly, while the stored catalogue schema stayed at 2. postcard is positional, so
      rows stamped 2 exist in four lengths and this build could read only the newest. The constant
      that number lives on says in its own comment that it must be bumped whenever `ItemWire`
      changes, and warns that raising it without a decoder is "the mistake it exists to prevent": it
      was read and ignored three times.

      Found by walking the back office section by section, in the one section that had rows and no
      buttons. The product's own diagnostic was right and nobody had looked at it.

      The vintages are frozen in `core/src/protocol/mod.rs` and the decoder tries them longest first,
      taking only the one that consumes the whole payload: postcard does not complain about bytes
      left over, so a shorter shape reading a longer row succeeds and silently drops the fields it
      has no room for. `CATALOGUE_SCHEMA` is 3 from here on. The fixtures are this shop's own bytes,
      lifted out of Postgres with `encode(payload, 'hex')`, and the guard counts `ItemWire`'s fields
      against a number beside the schema, because a comment that has to be remembered is not a rule.

      Walked: the back office's "price changes that never reached the counter" section is gone, and
      a device enrolled after the fix gets all seventeen rows. A till that had already advanced its
      cursor past those rows stays short of them, which is the loss the skip was designed to accept:
      one catalogue change rather than every till stopping for ever. The shop's remedy is the one on
      that screen, saving those items again

- [x] The other half of the same mistake is now caught: a shape that changes while its schema
      number stays put. `bytes_from_before.rs` freezes what older builds wrote and proves this one
      reads them; nothing watched what this build writes today, so a field appended to the standing
      state or to a sale would make every file already on a device unreadable the moment it shipped,
      and the device holding one is a till with a shop's unsent sales in it. That is exactly what
      happened to the catalogue, three times.

      So the bytes this build writes are frozen too, for the standing state and for a sale, built by
      decoding the version before and re-encoding: the fixture is the same shop the older fixtures
      describe, carried forward, and it holds one of everything, a parked basket and its line, the
      person at the till, the drawer they counted, the customer, the credential, the trail entry and
      an item, so a field added anywhere below the surface moves the bytes. The failure message says
      what to do rather than that something differs.

      What it cannot see, tried and written into the file rather than claimed away: two fields of
      the same type, side by side, holding equal values in the fixture, swapped

- [x] The wire has the same guard as the disk now. A field appended to a request makes every body
      an older build sends undecodable, because postcard is positional: the server does not see a
      missing field, it sees rubbish and answers "malformed". The rule is written at the top of the
      protocol module and it was walked past twice this month, once when three refusals gained
      figures and once when the trail gained the receipt a reprint was of. A person caught both, and
      one of those people was a reviewer rather than the author.

      Every shape in `core/src/protocol/mod.rs` is now written down in `protocol_shapes.txt`, name
      and fields, one per line, and the test fails when one moves: the message says to raise
      `PROTOCOL_VERSION`, freeze the old shape and decode both, and the record's own diff is what a
      reviewer reads to see what changed. It fails on legitimate changes too, which is the point.
      Broken with exactly the change that slipped past this month, and watched to name it

- [x] A backup written before today's fields existed is now proved to restore. The bundle is JSON,
      so a field added since is absent and takes its documented default, and every test in that file
      built its bundle with today's code: a field renamed or taken away would have passed all of
      them and broken every backup a shop has ever taken, which is the file somebody reaches for
      after losing the machine. The fixture is written by hand in the shape the older build wrote,
      and it is a shop's whole life in six lines: the shop, a till with its receipt block, a sale,
      somebody who buys on account, one line of the trail, and a trailer that counts them.

      It came back with the defaults that build would have given: no BIN, no wallets, a stock rule
      of nothing, nobody capped, and a reprint that names no receipt. Broken by renaming what one
      field is called on the wire while leaving the Rust name alone, which is the silent version of
      this mistake, and watched to fail with the line number

- [x] The back office was walked section by section against the database, which is what turned up
      the catalogue rows nobody could read. Everything else ties out: the day's takings come to
      83,054.43 over 29 sales with one refund of 57.50, and the same query against Postgres gives
      the same three figures to the poisha; the drawer figures, the deliveries, the suppliers, the
      tills and the account balances all read as what is stored.

      Three things looked like defects and were not, which is worth writing down because each cost
      time. A day report that said "nothing rung on that day" was a date box I had set from the
      console: the state behind it never moved, so it was answering about today, correctly. A till
      that would not block past the shelf was the back office sitting in a different shop, because I
      issued an owner code for a tenant id picked out of a database holding hundreds of test shops.
      And three held sales whose reason reads in raw milliseconds are rows written on 8 September,
      before the commit that fixed exactly that wording; the sentence this build writes says "rung
      56 years before it reached the shop".

      The lesson for the next walk: in a dev database, check whether an artefact predates the fix
      before treating it as a defect

- [x] A shop can send the whole list to its tills again, which is the other half of the catalogue
      repair. A till follows the catalogue by a cursor, so a row it passed over is a row it is never
      offered twice: making those seven rows readable again fixed every device enrolled afterwards
      and left the tills that had already gone by them short, with nothing on any screen to say
      which items. The advice the screen gave was to type the prices in again, one at a time.

      Every item's current state goes back into the log under new sequence numbers, upserts and
      tombstones alike, so a till that missed a withdrawal stops selling it too. The stored bytes
      are copied rather than decoded and rebuilt: the row this exists for is the one an older build
      could not read, and re-encoding it would either fail or change what it says. One row per item
      rather than per change, so a shop that has corrected one price fifty times sends one.

      The button is under what is on the shelves rather than beside the list of unreadable changes,
      because that list empties the moment the shop can read them again while the tills are still
      behind. It is also the answer for a till that was wiped or has been off for a month.

      Walked: pressed once, "15 item(s) sent to the tills again", and the till that had never seen
      the seed catalogue now finds "Sugar 1kg চিনি ১ কেজি 125.00", one of the seven rows no till
      could see this morning

- [x] Selling with the shop unreachable was walked end to end, which is the promise the whole
      product is arranged around and had not been checked in this state today. The server was
      stopped, and the till noticed within seconds and said so with the backoff; a cashier signed in
      with the line down, because the people are on the device; three sales were rung, each printed
      with a real receipt number out of the device's own block, T5A9-000008 to 10, with what is
      waiting to send climbing 1, 2, 3 and the numbers left falling 493 to 490. The server came
      back, "try now" was pressed rather than waiting out the backoff, and what was waiting went to
      nothing. All three are in Postgres with their numbers, their totals and the times they were
      rung rather than the time they arrived, none of them held for anybody to look at.

      One thing the walk found: the first failure of an outage read "আটকে আছে: Failed to fetch",
      which is the browser's two English words inside a Bangla sentence, on the one screen state
      this product exists for. A round that cannot reach the shop now says so in the shop's own
      language, matched on what Chrome, Firefox and Safari each call it, and anything with a name of
      its own is still left to the dictionary because a coded refusal says more than this could.
      Walked again with the server down: every state the line passes through is Bangla

- [x] A return could not tell zero rated from exempt. The shop's own figures are grouped by the
      kind of supply for exactly that reason, and the wire has always carried it: this crossing into
      the screen dropped it, so every line read as a percentage and two lines that are both nothing
      both read "0%". A shop declares those in different places on a return, which is the one thing
      that screen exists to tell apart. The sixth time this month that a lower layer was careful and
      the last hop threw the care away, and the second in the same file.

      Walked: a zero rated item added in the back office, sold at the till, and the return now reads
      "Zero rated 90.00 sold · 0.00 tax · 1 sale(s)" above "15% 72,901.25 sold · 10,935.18 tax · 32
      sale(s)", which is what Postgres holds to the poisha. The receipt says "Zero rated 0.00" on
      its own line rather than a rate of nothing

- [x] The defect that keeps happening now has a test, built the way the abandoned attempt said it
      would have to be. A reply carries rows, an arm decodes that reply and builds the rows a screen
      holds, and every field on the wire row must have a home on the shape the screen is handed. The
      pairs are read out of the code that does the work rather than written down, so they cannot go
      stale: an arm that stopped building a row would stop compiling.

      Ten fields are written down as deliberately not carried, each with what a shopkeeper loses by
      it: ids whose names travel instead, a reason carried as its code and figures, and three
      figures nobody has asked a screen for yet. Ten more replies are written down as converted
      somewhere else, naming where, because "something else does it" is exactly what a dropped field
      looks like from here.

      Put back the defect it was written for and it names it: "VatRowWire.supply (in the VatResponse
      arm)". The half-way state needs nothing from it, because a field taken off one of these shapes
      while the code still fills it in does not compile; what compiles, and what happened twice this
      month, is a field that arrives and is never mentioned at all.

      Three heuristics were tried and thrown away first, and what killed each is written in the
      commit before this one: a name search that reads past its own defect, an arm search that
      misses the rows, and a helper-following search that flags a dozen fields read perfectly well

- [x] The screen said a code lasts an hour and asked the shop for fifteen minutes. Its own request
      carries `valid_for_seconds: 900` and the sentence beside the code said an hour, so a
      shopkeeper who read it, walked to the back room to set a till up and came back twenty minutes
      later found a dead code and nothing saying why. The shop has always answered with how long it
      issued one for; that answer now goes on the screen, and the sentence about an hour is gone.
      Walked: "For Walk Ceiling Counter. Good for 15 minutes. Shown once".

      And the tills list says when the shop took each device on. That list is read when a device is
      to be cut off, and the question then is which of two tills with similar names is the one
      somebody enrolled last week: the shop has always known and no screen said. Both fields were on
      the wire already, which is how the guard from the commit before found them

- [x] Every shelf figure now says whether anything stands behind it. The wire has carried when an
      item was last counted since counts existed, and its own comment says absent means the figure
      rests on no count "which a shop should be told": no screen told them. A line reading "0 on
      hand" means one thing when somebody counted the shelf this morning and another when it is
      deliveries and sales added up since the day the item was typed in, and a shop cannot act on
      the second the way it would act on the first.

      Walked: every "Walk" item reads "never counted: this is what the books say, not the shelf",
      and the one item this shop has ever counted reads "61 on hand · counted 09/09/2026". The
      clearest line is "Walk Two Atta 2kg · -398 on hand · never counted", which is the four hundred
      sold past the shelf during this morning's walk and nothing counted against it since

- [ ] A scan for the defect that keeps happening was tried and set aside, and this says what was
      learned so the next attempt is better placed. Six times this month a lower layer was careful
      and the last hop threw it away; twice it was exactly one shape: a field on the wire that the
      bindings' copy of that shape did not have, so the reply decoded, the field vanished, and
      nothing complained. Both were found by a person looking at a screen.

      A bare search for the field name anywhere in the bindings passes, because `supply` and
      `receipt_no` are ordinary words in a file that also handles items and sales: the first draft
      read past both defects it was written for. Narrowing to the arm that decodes that reply works
      for the reply's own fields and misses the rows it carries, which is where both defects lived.
      Following the rows finds them, and then flags every field of every shape converted by a
      helper: sixteen fields of an item read perfectly well by one converter written once. Following
      helper bodies by name brings it down to a dozen, of which eleven are read somewhere the
      heuristic could not see and one is genuinely unread.

      What would work is not a heuristic: pairs, the bindings' own shape beside the wire shape it
      mirrors, asserting the first has every field the second does. Built in the commit after this
      one, with the pairs read out of the code rather than written down.

      The one thing the scan turned up that nothing reads is `TerminalHealthEntry.enrolled_at_ms`:
      the shop knows when each device was enrolled and no screen shows it. That is a missing line on
      the tills list rather than a defect, and it would help somebody deciding whether a device is
      one they still recognise

- [x] Two answers to what is on a shelf, and the till believed whichever arrived last. A catalogue
      row carries `on_hand_milli` and a stock answer carries the computed figure, and
      `Replica::upsert` took the catalogue's while `apply_on_hand` takes the shop's. Nothing in the
      back office ever puts a real figure on a catalogue row: the new-item form sends zero, a file
      import sends zero, and every other save forwards whatever the row already held. Deliveries do
      not touch it either, because goods receipts are movements. So the catalogue's figure is a
      fossil, usually zero, and the next catalogue edit of an item, a price correction or a barcode
      added, reset that item's shelf figure on every till until the next lap of stock, up to five
      minutes. Under the rule that stops a sale, that item could not be sold in that window without
      a supervisor; under the softer rule the cashier was told the shop had none of something the
      shelf was full of.

      Deferred twice as a design decision about which of the two answers wins, on the grounds that
      the losing one is what a shop restating stock from the back office would use. It is not a
      decision, and that is what took two deferrals to see: restating stock from the back office is
      a count, the count sheet exists, and a count goes into the ledger the shelf answer is computed
      from. The catalogue's figure is not the other answer to this question. It is a field nothing
      writes.

      So a catalogue change no longer says anything about a shelf: an item this device already holds
      keeps the figure it has, which already includes whatever this till has sold since. A new item
      takes what the row carries, because there is nothing better. The test that was said to block
      this, `once_the_server_has_the_sales_its_figure_is_taken_as_given`, turned out to be about
      double-counting settled sales rather than about the catalogue: it asks the same question
      through `apply_on_hand` now and still passes. One other test moved the shelf by pulling a
      catalogue row, and moves it the way the shop moves it instead.

      Walked live, with the shop set to refuse: a till holding sixty-one of an item was sent a
      catalogue change for it (written straight into `catalogue_change`, because the back office
      could not be opened), the change arrived, the item's name on the screen changed with it, and
      the shelf figure survived. The row carries zero, so before this the first scan after that
      change would have been refused

- [ ] Each staged build leaves the back office's old copy behind until that tab takes the new one
      over, and a tab left open across several deploys holds several: three copies of a 1.5 MB wasm
      were on the device during this session. It self-corrects the moment the tab takes over, which
      is the design, but a back office left open for a week of daily deploys is carrying the week

- [x] A till that has not learned the shelf treated the shop as empty, and under the rule that stops
      a sale that is a new till refusing everything scanned at it. A till learns what the shelves
      hold two hundred items at a time, five minutes apart, so a shop of two thousand lines takes
      fifty minutes to go round once. Until an item's turn came the till held whatever its catalogue
      row carried, which is usually nothing, and nothing read as none.

      Found by walking, not by reading: a till enrolled two minutes earlier warned "the shop has 0,
      this wants 1" about an item the shop had sixty-one of. The warning is the mild version. With the
      rule set to refuse, the same till refuses every scan of everything it has not been told about,
      which on its first morning is nearly everything, with a queue in front of it.

      So the shelf says nothing at all until the device has been round once: no warning, and above
      all no refusal. A rule that stops a sale has to rest on a figure somebody stands behind, and
      before the lap there is no figure, only an absence shaped like one. The driver now answers
      whether the window it just fetched closed a lap, the till writes that down, and it survives a
      reload, because a till that forgot would switch its shop's rule off for another lap on every
      restart.

      Deliberately not the other half of this question: which figure wins when the catalogue carries
      one and the shelf sweep carries another is still open below, and this does not touch it. What
      changed is when the rule may act, not which answer it acts on.

      A lap only counts when the catalogue was the same at the end of it as at the start. Without
      that, a till on its first morning closes a lap over the eight items it happened to hold while
      the rest of its catalogue is still arriving, and starts refusing sales of everything else. Seen
      on the walk: the lap closed ten seconds after enrolment.

      Walked end to end on a freshly enrolled till against the running server, with the shop set to
      refuse. At enrolment it read "learning the shelf"; the lap closed a few seconds later and the
      note went; the basket then took sixty-one of an item the shop holds sixty-one of, and the
      sixty-second was refused with the supervisor panel. Sixty-one is the shop's own figure, count
      barrier and all, and the catalogue row for that item carries zero: the till is refusing on the
      figure the shop sent it rather than on the one the catalogue fossil carries.

      `TERMINAL_SCHEMA` is 17, with version 16 frozen and a fixture of its bytes: a device upgrading
      says it has not been round the shelf, which is the safe end. Five nested shapes were frozen as
      that schema wrote them, because `frozen_shapes.rs` refuses a legacy state that names a shape
      still growing, which is the guard doing its job. Both screens say what is happening: the till
      shows "learning the shelf" while its shop's rule is on and its figures are not in, and the
      rule's own panel in the back office says a till starts doing this once it has been round.

- [x] The claim that a till had been round the shelf outlived the figures it was a claim about.
      Written down in the standing state so a reload would not switch a shop's rule off for another
      lap, on the assumption that the figures survive a reload. They do not: the shelf answers live
      in the replica, and the replica reaches the disk as a snapshot rewritten only when the delta
      log has grown, so a sweep changes no catalogue rows and saves nothing by itself. A till that
      came back from a reload said it knew the shelf and held the catalogue's own figures, which are
      zero, and in a shop whose rule says refuse it turned away everything scanned at it: the exact
      defect the day's work was about, resurrected by a restart.

      Found by writing the test that says the figures survive, and watching it fail. Sixty-one went
      in, forty came back, which is the catalogue's number.

      So going round is a fact about this run and is not written down. A reload costs one lap of
      silence about the shelf, and the screen says "learning the shelf" for the whole of it, which
      is the end of this a shop can see. The other end, a till refusing sales on figures nobody sent,
      is the one nobody can see. `TERMINAL_SCHEMA` is 18 with 17 frozen and its bytes in the
      fixtures, so the day's worth of devices that wrote the claim down are read and the claim is
      dropped: no build believes that about a device on the strength of what an older one wrote.

      Considered and not done: making the figures durable instead, by folding the replica into a
      snapshot when a lap closes. It is a few lines, and it would make the rule bite on figures from
      whenever the last snapshot happened to be written, which after a long shutdown is old. Stale
      and invisible is worse than silent and announced

- [x] Two of the shop's own figures for one drawer differed by 57.50 and the screen's only
      explanation pointed at the cashier. The back office shows what a till expected to hold and
      what the shop's own sales say the same drawer should have held, and the note under them named
      one cause: a till that has not finished sending. Found in this shop's own server log, twice:
      "a till's expected drawer disagrees with the shop's own sales, till_said=231750,
      sales_say=237500".

      Neither figure was wrong. A refund of 57.50 rung on that till was struck out afterwards, and
      the two answers part company for good the moment that happens: the shop's recomputation leaves
      a struck-out sale out, and the drawer keeps what the evening recorded, deliberately, because a
      duplicate that inflated what the till expected is exactly what that evening was short by.
      Rewriting the expectation now would erase the evidence, which the store's own comment says.

      So the gap is meant to be read, and until now nothing gave a person what it takes to read it.
      An owner whose till has finished sending was pointed at whoever counted the drawer for a
      difference the back office made. A closed drawer now carries how much of that window's cash
      belongs to sales the shop has since struck out, and the screen names it before the general
      advice.

      `PROTOCOL_VERSION` is 6, with the drawer, the push and the reply frozen as versions 2 to 5
      sent them, decoded at both ends: a till a release behind still hands over what it counted, and
      a back office a release behind still reads its drawers. Shown as the difference it explains
      rather than as the cash those sales moved, because a struck-out refund moves the two figures
      apart in the other direction: the shop carries the fact, the screen does the arithmetic.

      Walked live in the shop where it was found: "Your own sales for this till come to 2,375.00, not
      2,317.50. 57.50 of that difference is a sale you struck out afterwards."

- [x] Walked the two reports nothing had checked live, and both were right. What the shop owes the
      revenue for September: zero rated 90.00 on one sale, 15% on 72,901.25 with 10,935.18 of tax
      across 32 sales, which is the ledger's own answer to the taka and the sale, struck-out sales
      left out of both. And the paper: every screen asks the core for a paper in the core's own
      English, which is the standing decision, with its three reasons written at each call site and
      `paperWords` still without a caller.

      Nothing to fix in either, so what came out of it is a guard: a test now fails if a screen asks
      for a paper in anything but English. The reasons live at the call sites where somebody wiring
      the language in would read them, and now the day the raster path exists is a day somebody has
      to change a test on purpose rather than a comment they can walk past. Broken deliberately and
      watched to fail

- [x] Nothing had measured what a real catalogue costs the device that has to hold it. The replica's
      own comment says writing a catalogue one record at a time costs 1.5 s on a desktop and an
      estimated 7 to 20 s on a cheap tablet, which is why a page of changes is applied as a batch;
      the estimate had never been checked and nothing would have noticed it coming back.

      Measured, on ten thousand items, in release on this machine: building the search index 14 ms,
      ten thousand scans 0.9 ms in all, a thousand searches on a hot prefix 7 ms, a page of two
      hundred catalogue changes 14 ms, writing the snapshot 2.5 ms for 932 KB, reading it back
      2.8 ms. A cheap tablet is perhaps ten times slower, and the shop this is for has hundreds of
      items rather than ten thousand, so the shape holds: a scan does not depend on the size of the
      catalogue, and the only thing that costs real work is the index, which is paid at boot and
      once per page.

      Kept as a test rather than a note, with bounds loose enough to be about shape rather than
      speed, and asserted only in release because a debug build is a different machine. Broken
      deliberately by rebuilding the index per change instead of per batch, which is the trap the
      comment warns about: 70 seconds instead of 14 milliseconds, and the guard says so

- [x] Four kinds of override in a real shop's trail read as "something this build does not know
      about". Found by running the product's own diagnostic, `who_allowed_it`, against the live
      shop: entries written this week came back unnamed. The trail is what an owner reads when they
      want to know what happened at a counter that evening, and it was answering that question with
      a shrug.

      The screens were right. A till writes fourteen numbers into its trail, the frozen list holds
      fourteen, and the dictionary says all fourteen in both languages, all of which a test already
      holds. What nothing held was the other direction: every place that turns one of those numbers
      back into English is a hand-written list, and the one in `who_allowed_it` had stopped at nine
      while the shop was writing fourteen. The bindings' list, which is what a screen falls back to,
      happened to be complete.

      So the test now reads the frozen list and checks that everything which says a trail number in
      English knows all of them: the bindings and the example. Broken deliberately in each, and
      watched to name the number and the file. Walked afterwards against the live shop: "more sold
      than the shop has", "sold to somebody already past what they may owe" and "printed a receipt
      again", where four lines of shrug had been

- [x] External review of the day's eleven commits, which found one thing a walk could not have. A
      lap of the shelf counted as whole when the catalogue held the same number of items at the end
      as at the start, and the number of items is not the catalogue: the shelf is asked about by
      position, and withdrawing an item moves the last one into its slot. So a shop that withdrew
      one item and added another during a lap left a shelf nobody had asked about sitting in a slot
      the lap had already gone past, and the till would have said it knew the shelf and refused a
      sale of that one item. The replica now counts the times it gains or loses an item, a price
      change moves nothing, and a lap answers for the catalogue it set out round. Broken back to
      counting items and watched to fail.

      The review's other finding is not one, and there is a test saying why: `remove` leaves an
      item's local stock adjustment behind, which is right. The adjustment is what this terminal has
      sold and not yet sent, the id is the same item throughout, a withdrawal is a shop deciding not
      to sell something rather than the item's history being deleted, and the shop's figure still
      excludes those sales. Dropping it would make the shelf read high by exactly what the till sold
      in the outage. It cannot linger either: the map is cleared when the outbox drains.

      Nothing else: the version dispatch at both ends, the deferred wording and the struck-out cash
      sign were all read and found sound

- [x] A security review of the shop boundary: no way found for one shop to read or write another's,
      and four things inside a shop worth fixing. The isolation itself was read and found sound, and
      the reasons are written down: every business query runs in a transaction scoped to the tenant
      from the credential, the body's tenant is checked against the credential rather than trusted,
      every back office route is owner-gated, an enrolment code cannot be steered into another shop
      or made to grant a role above the caller's, tokens are 256 random bits stored as hashes and
      redacted from logs, and startup refuses an application role that could bypass row level
      security.

      What it found:

      A credential replaced came back as a till. The renewal insert wrote every column but the role,
      so the replacement took the column's default: an owner renewing lost the back office, eleven
      months after enrolling, with nothing to connect the two. Nothing caught it because the
      in-memory store keeps the whole caller and only Postgres drops it, which is the two-stores trap
      this codebase has been bitten by before.

      A device withdrawn mid-renewal kept working. The credential is authenticated, then replaced;
      an owner withdrawing the device between the two left the withdrawal undone. The replacement is
      now made out of the row being replaced, in one statement, only while that row is still there
      and unrevoked, and the device is told its credential is no good rather than that the shop is
      briefly unwell.

      A till could reprice the whole catalogue. The route that takes items a till wrote down at the
      counter took any id, including ones the shop already held, so any till could rename, re-tax or
      reprice anything by sending back what it had pulled. That is what the roles were added to stop.
      A till may now write down what the shop has never heard of, and an item the shop knows is
      acknowledged and not overwritten: acknowledged rather than refused, because a till holds an
      item until the shop says it has it.

      A till could take an owner's credit cap off anybody. It sends no cap, so the plain write put a
      zero over one and zero is no cap; the same write could flip whether somebody may buy at all. A
      till's write now keeps both of the owner's decisions and changes only the name, the phone and
      the BIN.

      Each fix has a test that fails without it, and the two credential ones live in the Postgres
      suite because that is the store that was wrong

- [x] Ten percent off the ticket was not ten percent, in a shop that prices on the packet. A rate off
      the whole ticket was shared out against each line's net, and an inclusive line's net is the
      shelf price with the tax taken back out and rounded: a rate against that rounded figure is not
      the rate the shopkeeper said, and the tax then goes back on the rounded remainder. An item at
      1.04 with fifteen percent inside it, ten percent off the ticket, came to 0.93 with 0.09 off,
      where ten percent off the same line came to 0.94 with 0.10 off. Same words, two prices, and
      the customer short by a poisha every time.

      The file's own note beside the recomputation says the two must agree, in as many words: "it
      would also make a ten percent line discount and a ten percent ticket discount on the same
      single-line basket produce different totals, which a shopkeeper checking the arithmetic with a
      pen finds immediately". They agreed in one of the two pricing modes, and the test that says so
      only ever tried that one.

      A rate off the ticket is now that rate off each line, taken on the same amount a line discount
      is taken on: the line is worked out again with the two added together, so there is one
      rounding rather than three. An amount off the ticket is unchanged, and still comes off the
      net, which is the older decision with its own test and its own reason.

      Found by an external review of the money arithmetic, then measured with the product's own
      code before touching anything. The property test that covers this now says what the fix
      restores: what a customer pays for a line priced on the packet is the price on the packet less
      whatever was taken off it, wherever it was taken off

- [x] Two more from the money review, both in what the shop does with a payload it has just found
      wrong.

      The shop stored the till's claimed total even when it had recomputed a different one. Every
      other figure beside a sale is the shop's own reading of the lines: the tax it declares, what
      left the shelf, what went into the drawer. The total was copied from the payload, so a sale
      claiming a hundredth of a taka went into the day's takings as a hundredth of a taka while its
      tax said 64.50 and its stock said one bag of rice. It is quarantined either way, so somebody
      does look, but the books held a figure the shop had already decided was wrong. The claim is
      not lost: it is in the reason beside the shop's own figure, and the payload is kept whole.

      And change could be handed back against a promise. A till caps change at the cash tendered and
      says why: banknotes out of the drawer for an account, a card or a wallet is real money against
      a promise, and the customer owes for it as well. Nothing at the shop end checked, so a payload
      altered after the till wrote it could say it and the arithmetic still added up, because the
      change had been added to the promise. A shop would find a drawer short, an account charged the
      whole amount, and a sale that looked ordinary. A refund's cash tender is negative, so the
      check is only about change actually given: comparing without that made every refund suspect,
      which is how the first version of it failed three tests

- [x] A refund gave back a poisha less than the sale took, on a basket whose ticket discount would
      not divide evenly. The remainder rule hands the odd poisha to the largest line, and it decided
      which was largest by the signed net: a refund's nets are negative, so on the way back the
      poisha went to the other line. Two lines at 1.00 and 1.01 with seven poisha off the ticket
      were charged 2.24 and refunded 2.23, and the tax declared and the tax taken off differed by a
      poisha on the pair.

      The same file already says the rule, two functions up, where a discount is worked out on the
      magnitude and given the sign of the base "because a return has to be the exact mirror of its
      sale". The ordering was the one place that did not follow it. Measured before and after with
      the product's own code, and broken back to the signed ordering to watch the test fail.

- [ ] A listed-price line declares a tax row whose own arithmetic does not work. With
      `VatBase::Undiscounted`, which is the regime where a discount comes out of the shop's margin
      rather than off the tax, a line of 100.00 with ten percent off declares net 90.00 and VAT
      15.00 at a rate of 15 percent. Fifteen percent of 90.00 is 13.50. The tax is right for that
      regime, the consideration is right, and the row is missing the third figure: the amount the
      rate was charged on, which is 100.00.

      Not fixed, and this is why. Which figure a Mushak 6.3 return wants in that column is a
      question about Bangladeshi VAT law, and this project's note on NBR sources says the ones found
      so far are vendor blogs and that nothing is to be published as compliance without a primary
      source. Carrying the taxable amount as a third figure is the shape that loses nothing, and it
      is a wire change, a storage change and a screen change that should be made once against the
      real rule rather than twice against a guess. Raised by the money review; measured with the
      product's own code so the numbers here are the product's own

- [x] A refund gave back what the catalogue says today rather than what the customer paid. A refund
      at the counter was rung by scanning the goods again, so the lines were priced out of the
      catalogue: a basket sold with ten percent off the ticket came back at full price, and an item
      whose price had moved since came back at the new one. The shop's own guard catches a whole
      basket coming back for more than it went out for; one line of it fits under the sale's total
      and passes without a word. Raised by the money review, which found the partial case; the whole
      case was worse and nobody had walked it.

      The paper the customer is holding says what they paid, so the till asks the shop for it. That
      needed four things: a route a till may call, because only the back office could look a receipt
      up and the person handed the paper is a cashier; the item on each line of that answer, so the
      goods go back on the right shelf, which is `PROTOCOL_VERSION` 7 with the shapes before it
      frozen; a way to ring a line back at what it was charged, which is refunds only because on a
      sale it would be a price typed over the catalogue's without the permission that guards one;
      and the screen that lists what was on the paper and asks how much of each is coming back.

      One thing found while walking it, which no test had: a looked-up receipt priced its lines one
      at a time, so a ticket discount belonged to none of them. Every line showed at full price
      under a total ten percent lower, on the back office's screen as well: the lines did not add up
      to the total the shop itself was showing. A receipt is now priced as the whole ticket it was.

      Walked end to end against the live shop: one item at 50.00 with ten percent off the ticket
      sold for 51.75, brought back off its own receipt as -51.75 with 5.00 of discount reversed, and
      the shop took the refund without holding it. Rung the old way it would have been 57.50, which
      is 5.75 of the shop's money

- [x] Three things the refund walk turned up, all of them silence.

      A receipt the shop does not have said nothing. The number may be mistyped, or the till that
      rang it may not have reached the shop yet; either way the cashier is about to scan the goods
      instead, which prices them at today's catalogue rather than at what this customer paid. The
      screen says which of the two it is doing now, because the difference is money.

      A receipt somebody has already had money back against said nothing either. The shop refuses
      more than the whole of it, but that refusal arrives after the money has left the drawer, so
      the person deciding is told before rather than after.

      And a refund started by mistake could not be left. The till stayed in refund mode with nothing
      on the ticket, every scan came back as goods returning, and the only way out a cashier had was
      to reload the page. Walked into it while walking something else, which is how it was found.

      All three walked live: "The shop has no receipt TX-999999", "51.75 has already been given back
      against this receipt", and the way out appearing on an empty refund and putting the till back
      to an ordinary sale

- [x] A looked-up receipt showed the payload's own summary rather than the shop's reading of it, and
      its lines did not add up to its total. Two halves of one thing, both found while building the
      refund that reads a receipt.

      The lines were priced one at a time, so a discount taken off the ticket belonged to none of
      them: every line showed at full price under a total ten percent lower, on the back office's
      screen as well as the till's. A receipt is priced as the whole ticket it was.

      And the sale's own net, tax and discount were read out of the payload, which is the one
      summary a shop has already decided about: those figures are checked when the sale arrives and
      the sale is quarantined when they disagree, so showing the assertion afterwards is a shop
      reading a figure it has already found wrong. They are the shop's own reading now, which is
      what the tax rows and the stock movements have always been.

      A receipt with ten percent off 430.00 now says 387.00 net, 58.05 tax, 43.00 off, 445.05 total,
      and one line of 445.05 that adds up to it. Broken back to line-by-line pricing and watched to
      fail

- [x] The shelf walk reported the shop's own rule as broken, and it was the walk that was behind.
      `past_the_shelf` hand-drives a till: it pulls the catalogue, asks what the shelves hold, and
      rings one more than there is. Since a till says nothing about the shelf until it has been round
      it once, and the walk never said the lap had closed, the rule stayed silent and the walk
      printed "rang one more: TAKEN, which is the rule not working". It now says the lap closed, as
      the server's own end-to-end test had to, and reads: "rang one more: refused, the shop has 40
      Soybean Oil 1L and this basket wants 41".

      Second time today an example went behind the product: the trail one had a list of action
      numbers that stopped at nine. Both were caught by running them against the live shop, which
      nothing does automatically, because they need a server and a shop with data. Worth running
      after a change to the core, and that is the whole of the practice: they are walks a person
      does, and their value is exactly that they are not tests

- [x] A review of the path that keeps a shop's sales safe found two ways to lose them, and both are
      fixed. It read the journal, the framing, the outbox, the driver, the commit path, the browser
      backend and the server's ingest, and said what makes the rest sound: the acknowledgement only
      advances over a contiguous prefix, a lost reply causes a resend the shop deduplicates on
      `(tenant, id)`, receipt numbers survive a crash because the standing state records the lease
      before the log is truncated, and the frames checksum their own headers and payloads.

      A sale waiting on a till that was upgraded was sent as something it is not. The outbox forgot
      which schema the frame was written under and stamped every payload with the number this build
      writes. A till offline across an upgrade holds sales the older build wrote: postcard is
      positional, so the shop could not decode them, kept the bytes as a repair nobody can read, and
      the till dropped them as sent. The goods, the tax and anybody's account went with them, and
      the only copy was on the device. The schema travels with the sale now, which is all the shop
      needed: it knows every shape its ancestors wrote.

      And a log that had gone bad in the middle was emptied under the sales behind it. What the
      outbox can see is what reads through, and a frame that rots in a live log hides everything
      after it: the outbox says there is nothing left to send, and the log is deleted. Anything that
      throws bytes away now asks whether the log reads through first. A log that does not keeps every
      byte, and the next open puts what nobody can read into the salvage file where a person can be
      pointed at it.

      Both broken deliberately and watched to fail, the second one twice: the first version of the
      test never reached the gate, because the drawer was open and a till with an open drawer never
      empties its log at all

- [x] Two more from the review of the path that keeps sales safe.

      Another device's standing state was read as this device's own. A log's frames are checked
      against the shop and the terminal that opened them, and a foreign one refuses to open; the
      blobs beside it were not checked at all. A `terminal-a.bin` copied off another till, or a
      backup of somebody else's device restored onto this one, was taken as this till's history, and
      that file carries the block of receipt numbers the other device was given: this till would
      print numbers that one has already printed, with the customer's copy across the counter before
      the shop ever sees two sales under one number. Checked now, on the way in and on the way out,
      the catalogue snapshot as well.

      And a sale already durable could still be reported as failed. The commit's own note says
      durable means the receipt may print, and what came after it could return an error: the drawer's
      arithmetic. The cashier would be told the sale failed, ring the basket again, and the shop
      would have two of it with one customer standing there. Nothing after the commit can turn a
      sale into a failure now. What that arithmetic can cost is this device's running drawer figure
      until the next boot, which rebuilds it from the same frames, and the till says so rather than
      swallowing it, because an evening's count against a figure that is behind is an argument
      nobody can see the cause of.

      The first fix was written in the wrong place first: the read path picks the newest slot itself
      rather than going through the recovery that chooses one, so the test still saw the foreign
      file until both were checked

- [x] A basket is rung under one id now, however many times somebody presses. A checkout can be
      durable and still come back as a failure: the sale is written and flushed, something after that
      fails, and the cashier is told it did not happen. They press again. The id was minted at each
      press, so the shop took two sales for one basket and had no way to tell they were the same
      one; with one id per basket the second press is a replay, which the shop deduplicates on
      itself and the id, and which its own tests already call ordinary.

      Raised by the review of the storage path, which found the durable-then-failed sequence in the
      browser backend: the flush barrier is global, the critical log is flushed first, and a later
      handle failing takes the whole commit down after the sale is already on the disk. Fixing the
      barrier means changing the storage trait, which is the most safety-critical interface here;
      one id per basket removes the harm without touching it, and is the thing that should have been
      true anyway.

      It is one line of a screen, which is why there is now a test that reads that line: the core
      mints no identity, the server sees two different ids and believes them, and no test of either
      would have noticed. The id ends when the basket does: rung, parked, or thrown away

- [ ] Dev residue from today's walks, in the demo shop and in this browser. Two more tills in the
      list, "Walk Words Counter" and "Walk Shelf Counter", the second enrolled with a code minted
      straight into `enrolment_code` because the back office could not be opened. Two orphaned
      stores in the browser's OPFS whose handles are still held by something that is not a tab. The
      shop's stock rule was moved to refuse for the walk and put back to warn afterwards. Two more
      back offices, "Walk Desk Two" and "Walk Desk Three", enrolled the same way for the same reason,
      and one item renamed to "Walk Barcode Z" and back again to prove a catalogue change had
      landed. A fourth till, "Walk Paper Counter", with one sale on it and a browser print dialog
      left open on that tab: the till prints by handing the browser its own dialog, which blocks the
      page until a person dismisses it. Then a day of walking refunds: four more tills, two more
      back offices, two hundred sales from `long_day`, a supplier paid, a sale struck out and put
      back by `rung_twice`, and a receipt sold and refunded to prove the money. The demo shop is a
      walked-over shop, not a clean one, and every code was minted straight into `enrolment_code`
      because this browser ghost-locks a store on reload and each walk needed a device of its own.

      Today's money walk adds four more: "Walk Money Till", "Walk Money Desk", "Walk Money Desk Two"
      and "Walk Money Till Two", enrolled the same way, plus a delivery of twelve Rice Miniket at
      380.55 booked against nobody, one sale of four at 430.00 with 100.00 off, and the refund of
      two of them. The codes were minted with a hash of the code as typed, which is wrong twice over
      before it works: the shop normalises what a person types, mapping L to 1, O to 0 and U to V,
      so a code containing any of those hashes to something the shop is not holding. Codes for a
      walk are drawn from the shop's own alphabet, which omits I, L, O and U for the same reason

- [x] A till could be told to close a window that is not open, and then it had nothing else to say.
      Met three times in one day of walking: one tab in the whole browser, and every file in that
      terminal's store answering `NoModificationAllowedError`, which the screen reads as "this page
      is already open in another window of this device". Nothing was. Something inside the browser
      was still holding the access handles minutes after the tab that opened them had gone. Proved
      rather than guessed: a probe worker asked for all three files by name and each refused.

      The wording is right for the case it was written for, and its only way out, close the other
      window, is a dead end when there is no other window. So the screen says the second thing after
      the first has been tried and failed: if no other window is open, the device is holding the shop
      for one that has gone, and switching it off and on is what clears it. Not a diagnosis, because
      nothing on the device can tell the two cases apart, and one instruction at a time, because the
      commonest cause really is the other window.

      Walked on both screens with a second tab open: the first message, "Try again", and after it
      failed the second instruction above the button. Broken deliberately, so the second advice comes
      first, and watched the test fail

- [ ] Five files are past the size a person can hold in their head, and the standing rule here is
      around two thousand lines. Measured today, production lines with the tests beside them taken
      out: `server/src/repo.rs` 4,840, `apps/admin/src/App.svelte` 4,529 with no tests in it at all,
      `bindings/src/lib.rs` 3,200, `server/src/http/back_office.rs` 2,780, `core/src/till.rs` 2,730.

      The back office screen is the one to split first: it is the largest with nothing beside it, and
      its seams are already drawn, one per panel. What has to move with it is the guards that read
      it by name, because four of them scan `apps/admin/src/App.svelte` for English prose, for keys,
      for commands and for the id a sale is rung under, and a screen split into files those scans do
      not know about is four guards that quietly stop guarding. They would read the directory.

      The back office screen is done: 4,629 lines to 1,988, in ten panels, with the guards moved to
      read the directory first and a new one that catches a panel using something nobody handed it.
      The bindings are done too: `bindings/src/lib.rs` 3,324 to 2,062, with the shapes that cross
      that boundary in `shapes.rs`. They are the contract, and a contract reads better when it is
      not interleaved with the code that honours it. One guard read the command enum out of
      `lib.rs` by name and had to be pointed at the new file; it says so where it reads, and its own
      assertion that the scan found more than thirty commands is what catches the next move.

      And the back office's routes: `server/src/http/back_office.rs` was 2,843 lines of handlers
      with 4,148 lines of tests under them. It is a directory now, split by what a shop is asking
      about: what it took and owes, the catalogue and the shelves, who the shop is and who may stand
      at a till, and everything that needs somebody to look. Each test went to the file holding the
      route it calls, which is a classification the tests made themselves: every one of them names
      its route in its own body. Eight that touch no route stayed at the root, and the two payloads
      more than one of them needs are in `proof.rs` beside them. No route changed and no caller
      changed, because the handlers are re-exported from the module root.

      And the till itself, which is the heart of the product and the file most often edited:
      `core/src/till.rs` 2,885 lines of one `impl` block over 3,150 lines of tests, split along the
      seams it already had section headers for. Booting from what is on the device and writing down
      what outlives the log; the catalogue and selling out of it; who is at the till and who buys on
      account; and the drawer. What stayed is the type itself, the refusals, the receipt numbers and
      the sync. Every test went to the file holding what it is named after, the fixtures are in
      `proof.rs` beside them, and thirteen that are about the whole till stayed at the root.

      One guard read the trail numbers out of `till.rs` and would have gone on passing while reading
      a third of them: the trail is written from the drawer, from the people at the counter and from
      selling. It reads all five files now. That is the third guard in two days that named one file
      and had to be taught the directory, which is worth saying out loud: a scan that names a file
      is a scan with a deadline on it.

      What is left: `server/src/repo.rs` at 2,156.

      What the screen split taught, for whoever does the next one: move the markup by finding both
      of its ends and checking what is between them, because cutting by index swallowed a panel tag
      that sat in the middle and the build stayed clean. And a panel is handed what it needs: four
      props were missed in one afternoon, every one of them silent

- [x] A fourth review, of the two screens rather than of the money or the shop boundary, and the
      fourteen findings it returned. All fourteen were real and all fourteen are fixed.

      Four were a screen sending less than it was holding. Stopping the sale of an item rebuilt the
      item from four fields, so an exempt item taken off sale came back standard rated with its
      category and its Bangla name wiped. Stopping an account sent no credit limit, and a limit
      absent is a limit of zero, so an owner pausing a buyer erased the cap they had set. Both now
      send the whole record. Booking a delivery minted its id inside the request, so a booking whose
      reply was dropped was booked twice when the owner pressed again, with the stock and what the
      shop owes its supplier counted twice with it; it now keeps one id until the booking lands,
      the same way a basket keeps one. And a supplier statement carried a "show older entries"
      button from the customer account beside it, calling a name that does not exist in that
      component: it threw where it stood, and it is gone.

      Three were a screen showing something that was not true. Enter in the tender box always rang
      cash, whatever the cashier had selected, so a wallet payment taken by pressing Enter was
      recorded as money in the drawer; it now takes what was chosen. A refund the till refused for
      want of a supervisor still put the old receipt's lines on screen under "bring these back",
      which is a promise the till had not made: what follows a refund now runs when the refund has
      actually started, and again after a supervisor allows it, and never while it is refused. The
      price check kept the last thing it was asked about, so a cashier who checked rice, went back
      to scanning and opened the price check again was shown rice until the next reply landed, with
      a customer standing there holding soap.

      Two were a form and a button. Adding a person cleared the name and the PIN whether or not the
      save worked, so an owner whose shop could not be reached watched a PIN they had just chosen
      disappear, and a PIN is chosen rather than remembered; the form is now cleared only when the
      person is saved, and the id is kept until then. "Print again" was the one button on the till
      not held while something else was in flight, so two taps wrote the shop two reprints and
      opened the print window twice.

      One was a count that could be thrown away by a press nobody meant. The second press that
      confirms it stayed armed when the owner left counting mode, so arming it, walking off to book
      in a delivery and coming back an afternoon later made the first innocent-looking press throw
      away the count. Every way into and out of that mode now goes through one place that disarms it.

      One was the takings under the wrong date. They were replaced only when the request worked, so
      an owner changing the date while the shop could not be reached was shown yesterday's figures
      under today's heading with nothing to say so. What is on screen now belongs to the day it was
      asked for, and goes when the date does.

      Three were the shop's money answered in JavaScript, which is the standing rule of this build
      and the reason to fix them whether or not a figure was ever wrong.

      The till was subtracting one figure from another to decide what was still owed and whether the
      sale was paid for, and deciding for itself what the difference meant. Two places deciding when
      a sale is settled is two answers the day either is reworded, and the one the customer is shown
      is not the one that closes the drawer. The rule now lives once, in `Cart`, shared by `close`
      and by a new `settled()`, and the view carries the answer and the outstanding amount with its
      sign.

      A partial refund off a receipt was split in the screen: what came off the line, times how much
      is coming back, divided by how much was on it, rounded by JavaScript. The whole line as the
      paper has it now goes across and the core takes the share, in `Minor::share_of`, rounded half
      away from zero like every other split here. Proved by a test that half a discounted line comes
      back with half of what came off it, and that a line returned in two halves and a line returned
      at once come to the same money; the split was then broken deliberately and the test named the
      failure.

      A delivery was read out of two text boxes with `Number()` and a multiply, where
      `Number("1e3")` is a thousand and `Number("1.005") * 100` is 100.49999999999999, and both
      reached the shop's stock and what it owes. It now uses the two parsers the rest of the product
      uses, and a quantity or a cost that is not one is refused where it was typed. The list of what
      came in was multiplying the lines out itself, so a delivery now carries what it cost in all:
      protocol 8, with the version 7 shape frozen and answered to any screen a release behind,
      because these bodies are positional and a v7 reader handed the total takes it as the start of
      the next delivery. The guard that reads every field a reply carries caught the total stopping
      at the bindings before a person did.

- [x] The review of those fixes, which found four more. Two of them were the same mistake wearing
      the other face: keeping an id for ever is as wrong as minting a fresh one at every press.

      A booking whose reply went missing leaves the form open. Press again unchanged and the shop
      reads the repeat it is; but change what is on the form first and the shop still sees an id it
      already has, calls it a repeat, and drops the goods that were typed over it. Worse for a
      person, because the shop upserts on that id: adding Amina, losing the reply, and typing
      Rahima over the same form renamed Amina rather than adding anybody. So the id belongs to what
      is on the form. `apps/shared/one_id.js`, tested, and used by both.

      The takings were cleared when the date changed and not when the date stopped being one, so
      clearing the field and pressing Look left the last day's figures sitting under a blank date
      beside the words "that is not a date". Walked: the figures now go with the date.

      And the delivery total came back as zero when the lines could not be added up in this money,
      which a shop would read as a delivery worth nothing. It is absent now, and the screen says so
      and points at the lines. That made the field optional, which is a shape change, caught the
      obvious way: the running server was still answering with the old shape while the browser read
      the new one, so the list came back empty and nothing said why. postcard is positional and
      that is exactly what it does.

      Walked live against the shop, end to end, and read out of Postgres rather than off the screen:
      four Rice Miniket at 430.00 with 100.00 off the line rang at 1,863.00 (net 1,620.00, VAT
      243.00). Two brought back off the paper refunded 931.50 (net -810.00, VAT -121.50): half the
      line, half the discount, and the tax reversing exactly. The delivery of twelve at 380.55 came
      back from the shop as 4,566.60 rather than being multiplied out on the screen. "1e3" typed as
      a quantity was refused where it was typed. The count's second press was armed, left for the
      delivery form, and came back disarmed.

- [ ] Splitting the back office screen, which is the oldest thing on this list and the one that
      makes every other change harder. Started, not finished: 4,629 lines to 4,046, with three
      panels in files of their own and the guards moved first.

      The guards came first because they are what makes a split safe. Seven of them work by reading
      a screen's own source, and every one of them named `apps/admin/src/App.svelte` and
      `apps/till-web/src/App.svelte` directly. A screen that outgrows one file is a screen those
      guards stop guarding, and they stop quietly: the named file still exists, still passes, and
      the half that moved out is read by nobody. That is the worst failure a guard has, because the
      suite goes on being green. A screen is a directory now, `apps/shared/screens.js` reads all of
      it, and there is a guard on the guard: both screens are found, neither comes back empty.
      Proved by writing a panel into a directory that did not exist an hour ago, with a date in
      Greenwich, English in a title attribute and English between tags: three guards named it.

      The look moved out with them, into `apps/admin/src/screen.css`. Svelte scopes a component's
      styles to the markup in the same file, so a panel moved out of App.svelte would have come out
      of the stylesheet with it and lost every rule the rest of the screen is drawn with. Walked:
      the panels are drawn exactly as they were, struck-through rows and red shortfalls and all.

      Out so far: the two drawer panels, the tills, and the two questions about a period. Each owns
      its own state and its own questions and is handed the way to ask the shop, the words, the
      money, and the screen's one message line. The list of tills stays with the screen, because a
      drawer and a sale carried in by hand are both named from it.

      Two things fell out of moving them. Cutting a device off said nothing at all: the sentence was
      written and then wiped by the read that followed it, so an owner who cut off a lost till was
      answered with silence. And the tax panel was adding up what a shop owes the revenue in
      JavaScript, over rows the shop had sent, which is the same mistake as the delivery totals and
      on the one figure an owner copies onto a return. The VAT reply carries its own total now,
      inside protocol 8 beside the delivery one, with the version 7 shape frozen. Walked: 24,660.78
      for the month, from the shop.

      The suppliers went next, which is three sections and one question asked three ways: who the
      shop buys from, what it owes them, and what has come in. 4,046 lines to 3,774. The list of
      suppliers stays with the screen because the delivery form picks from it, and booking a
      delivery asks the panel to read back what is owed, which is the first cross-panel call and
      was walked: two at 50.00 against Rahman Wholesale moved 400.00 to 500.00 and two deliveries
      to three. A third thing fell out of the move: a payment to a supplier kept its id for ever,
      so correcting the amount after a reply went missing would have been dropped as a repeat of
      the first. Same fix as the delivery and the person, from the same shared place.

      Then the accounts: who buys on account and what they owe, which are two sections and one
      book. 3,774 lines to 3,379. The khata page is handed back up to the screen to print, because
      what prints is the only thing on the page: the print rule hides `main`, and a panel inside it
      drawing its own paper would print nothing. Two more of the same two mistakes came out with
      it. A payment taken off what somebody owes kept its id for ever, like the supplier payment
      and the delivery and the person. And a credit limit was read with `Number(...) * 100` and
      rounded, on the figure that decides how far a cashier may let somebody go: "1e3" was a cap of
      a thousand taka nobody typed. Walked: "1e3" refused where it was typed, 1500.50 saved as
      150050 poisha and read back out of Postgres, 97.50 off what Karim Uddin owes leaving 400.00,
      and the khata page printed once from outside `main`.

      One thing the walk caught that nothing else would have. Carving the accounts markup out by
      its first and last section swallowed the drawer panel's tag, which sat between them, so the
      screen went blank with "Drawers is not defined" in the console. The build was clean, the
      guards were green, and every test passed: a component tag that moves into a file where its
      import is not is a runtime error and nothing else. Cutting markup by index is the thing to
      stop doing.

      Then the repair queue, which is the biggest single move: seven sections and one job. A
      receipt somebody has brought to the counter, the sales the shop could not take on trust, the
      answers already given, the gaps in the numbering, the sales carried in by hand off a device
      that cannot send, the items a till wrote down, and the catalogue changes no till could read.
      3,379 lines to 2,844. The markup was located by both its ends and checked for panel tags
      before it was cut, which is the answer to the blunder above.

      Walked: the queue lists five with their reasons worded from the figures inside them, a
      decision with no note is refused in the shop's own words, one answered leaves the queue and
      appears under what has already been decided with its note beside it, and a receipt looked up
      by number shows the sale rung at that till with the refund already given against it.

      Then what a day took, which is one question asked twice and moves on its own. 2,844 lines to
      2,701. Walked: the ninth reads 83,054.43 over 29 sales including one refund, today reads
      1,821.60 over three, and clearing the date takes the figures with it rather than leaving one
      day's takings under another day's heading.

      Then what sold, and then the shop's own list taken out and brought back. 2,701 lines to
      2,191.

      The import walk found the failure this split can produce and nothing else here catches. A
      panel's markup reads `money(...)`; the value used to be a variable in the same file and is
      now a prop, and the screen did not pass it. The build was clean, every test was green, and
      the panel threw where it stood: Svelte renders nothing for the block that threw, so what a
      shopkeeper saw was a preview that never appeared, with no message and nothing in the log
      that pointed at it. It took an afternoon to find by hand.

      Four of them were in the tree at once. The import preview printed money it had not been
      handed. The trail named a till from a list it did not have. The who-owes list called its
      loader by the name it had before the move. And an item a till wrote down opened a form that
      had stayed behind on the screen. `apps/shared/panels.test.js` now reads what each file's
      markup calls or indexes and what its script declares, and says so when the first is not
      covered by the second. Broken deliberately on each of the four; it names them.

      Walked after the fix: a one-row file read, matched, previewed, written, and the row found in
      the shop's own catalogue at 99.50 with its category and its tax.

      Then the form one item is added or corrected on, which the shelf list and the repair queue
      both open: correcting an item is the same act wherever it is started from, so there is one of
      it. 2,191 lines to 1,992, which is the standing rule met. Two more amounts came out of
      JavaScript with it: a price and a cost were read with `Number(...) * 100` and rounded, on the
      figure that ends up on a shelf label and in every sale of that item. Walked: "1e3" as a price
      is refused where it is typed and the form keeps what was typed, 45.75 saves, and pressing
      "correct it" on the shelf list opens the form with 45.75 and 30.00 in it.

      What is left in App.svelte: the shop's own settings, the people, and the shelf. Those last two share the catalogue this device holds, the
      shelf figures and what each item cost, so they move together or not at all. The shelf is the
      largest thing left and the target is around two thousand lines.

- [x] A barcode can be read with the tablet's own camera, for a shop with no scanner on a wire, for
      a second counter on a market day, and for the back office where nobody is going to buy one.

      Two things a camera needs that a scanner does not. The check digit is worked out here rather
      than trusted: a camera is held at an angle by somebody with a customer waiting, and a misread
      digit that a check digit would catch is a customer charged for something they are not holding.
      And the same code has to be read twice in a row before the till hears it, because one frame is
      a guess and the second costs a fraction of a second at thirty frames a second. A reading that
      fails its check digit is not even remembered, or the same misread twice would ring.

      The decoding is the browser's own `BarcodeDetector`, so nothing is fetched and the camera works
      with the internet down, which is the whole point of this product. Where it is missing the
      screen says so and the scanner box is still there. QR is deliberately not in the list: a QR
      code on a counter is a payment, not an item, and reading one into the basket rings whatever
      number a customer's phone is showing. The camera stops when the page is hidden: a browser
      hands a hidden page no frames, so it would be a light on the counter and a flat battery for
      nothing.

      The back office reads one too, on the form where an item is written down: a barcode typed off
      a box by hand is where the wrong digit gets in, and that is the screen where somebody is
      holding the box. The loop is shared rather than copied, because two loops would be two answers
      to when a reading is believed, and the whole point of reading twice is that the answer is the
      same both times. Walked: read twice, the box filled with 5901234123457, the camera shut
      itself, and the item saved and read back with that barcode on it.

      What it reads goes through the same door the scanner's digits go through, so the shelf rule,
      the refund and the price check are one path and not two. Walked with the camera and the
      decoder stubbed, this machine having neither: a code nobody's catalogue had came back as
      "no item in the catalogue has that barcode" with the offer to write it down, writing it down
      rang it at 40.83, reading the same label again made it two at 81.65, and a label misread by
      one digit was read three times and rang nothing. Untested: the tablet's camera itself, the
      browser's own decoder, and the frame callback, because a hidden tab is handed no frames and
      this one was driven hidden

- [x] The frozen list of refusals could not see a refusal nobody built. Both tests compared a
      hand-written `one_of_each()` against the frozen codes, so a variant missing from that list was
      invisible in both directions: it had a code the compiler insisted on, and nothing ever read
      it. No line in `refusals.json`, no word in Bangla, and a cashier refused for that reason
      reading English at the one moment they need their own language. Found by another session,
      whose refusal for a negative quantity passed the guard while carrying none of that.

      The server's half of this file already had the answer: scan the enum out of its own source and
      insist the hand-written list builds one of each. The till's half now does the same, over the
      five enums a till refusal is made of. It found three straight away, `Journal`, `Sync` and
      `Wire`, which a till wraps rather than raises itself: every one has a code and a word, and
      nothing had ever checked either. And a rate outside nought to a hundred percent, which a shop
      reaches by typing one into the item form.

      Proved by adding a refusal with a code and no builder: the guard names it. The compiler
      catches the other half, a variant with no code at all, because `code()` and `Display` are
      exhaustive matches

- [x] The workspace lints clean again, tests included, which it had not for some time. Two in
      production code: a plain `+` in the receipt's word wrapping, now saturating like every other
      count there, and a needless borrow. The rest were test files doing test arithmetic without
      saying so at the top, which is what the benchmark file has always done. `cargo clippy
      --workspace --all-targets` is the gate and it is at zero

## Next
- [ ] Implementation plan document, once more of the core shape is proven in code
- [ ] Decide whether the Android UI is Flutter at all. The C ABI removes the reason to prefer it
- [ ] Resolve open questions: NBR primary source, printer models to certify, Android distribution,
      DCO before first external PR, hosting substrate for the paid tier, browser storage backend
