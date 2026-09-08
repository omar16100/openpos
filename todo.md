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
- [ ] `may_void_line` is a permission nothing enforces: `Till::remove_line` takes a line off with no
      check at all. Enforcing it as written would stop a cashier correcting a mis-scan, which is
      worse, so the question is what the permission should mean in a design where nothing is
      committed until checkout. It is at least not a promise on a screen: the back office offers two
      role presets and never this flag on its own, so no shop has been told it does anything. It is
      carried on the wire and in the standing state and means nothing today, and either it gets an
      enforcement point somebody asked for or it comes off the operator record
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
- [ ] A shop that never counts its drawer never lets the log go. That is a shop with no Z report and
      no reconciliation, so it is a bigger problem than the disk, but the disk is the part this
      change makes worse: roughly 145 KB per thousand sales, kept until somebody counts
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
- [ ] The scan is text: it finds SQL by looking for `from sale` in a string literal. A query built
      by concatenation, or one that names the table some other way, would walk past it
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
- [ ] A shop with more than two hundred lines takes a lap of five minutes per two hundred to refresh
      the whole catalogue's stock, so the figure behind a refusal can be that stale for the items at
      the far end. The bound is the server: on-hand is one query per item, count barriers and all,
      and a set-wide version of that query is a second answer to the same question, which is the
      thing this codebase keeps refusing to build
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
- [ ] `catalogue/delete` is the other route nothing calls. Left alone on purpose: the back office
      stops an item being sold, which keeps its history, and deleting one is a tombstone that takes
      the history with it. Worth a screen only if a shop asks for it

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

## Next
- [ ] Implementation plan document, once more of the core shape is proven in code
- [ ] Decide whether the Android UI is Flutter at all. The C ABI removes the reason to prefer it
- [ ] Resolve open questions: NBR primary source, printer models to certify, Android distribution,
      DCO before first external PR, hosting substrate for the paid tier, browser storage backend
