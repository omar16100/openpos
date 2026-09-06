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
- [ ] Axum server: batch ingest, lease issue, tenant scoping
- [ ] `core::outbox` and `core::sync`: append-only ticket log, cursor pull, batch push
- [ ] `core::lease`: receipt number blocks with epoch fencing
- [ ] Axum server with Postgres, tenant scoping, batch ingest
- [ ] `flutter_rust_bridge` spike (gate on the Flutter till)

## Next
- [ ] Implementation plan document, once more of the core shape is proven in code
- [ ] Spike `flutter_rust_bridge` before committing the Flutter till
- [ ] Resolve open questions: NBR primary source, printer models to certify, Android distribution,
      DCO before first external PR, hosting substrate for the paid tier, browser storage backend
