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
- [ ] `flutter_rust_bridge` spike (gate on the Flutter till)

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
- [ ] Enrolment rate limiting collapses to one bucket behind a proxy (needs trusted-proxy config)
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
- [ ] Terminal tokens never expire and record no last-used time

## Next
- [ ] Implementation plan document, once more of the core shape is proven in code
- [ ] Spike `flutter_rust_bridge` before committing the Flutter till
- [ ] Resolve open questions: NBR primary source, printer models to certify, Android distribution,
      DCO before first external PR, hosting substrate for the paid tier, browser storage backend
