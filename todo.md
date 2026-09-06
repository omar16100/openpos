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
- [ ] `core::replica`: in-memory catalogue with barcode, code and token indices
- [ ] `core::storage`: snapshot plus delta log behind a five-operation trait
- [ ] `core::outbox` and `core::sync`: append-only ticket log, cursor pull, batch push
- [ ] `core::lease`: receipt number blocks with epoch fencing
- [ ] Axum server with Postgres, tenant scoping, batch ingest
- [ ] `flutter_rust_bridge` spike (gate on the Flutter till)

## Next
- [ ] Implementation plan document, once more of the core shape is proven in code
- [ ] Spike `flutter_rust_bridge` before committing the Flutter till
- [ ] Resolve open questions: NBR primary source, printer models to certify, Android distribution,
      DCO before first external PR, hosting substrate for the paid tier, browser storage backend
