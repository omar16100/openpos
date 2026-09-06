# openpos architecture (C4)

Status: current as of 2026-09-06. Architecture source of truth. Update on every architecture change:
containers, components, services, dependencies, data flows.

## Level 1: context

```
   ┌──────────┐        ┌──────────────┐        ┌───────────────┐
   │ Cashier  │        │ Shop owner   │        │ openpos cloud │
   └────┬─────┘        └──────┬───────┘        │ (paid tier)   │
        │ rings sales         │ stock, prices  └───────┬───────┘
        ▼                     ▼ reports                │ managed
   ┌─────────────────────────────────────┐             │ upgrades,
   │            openpos                  │◀────────────┘ backups
   │  till (Android/browser) + server    │
   └───┬──────────────┬──────────────┬───┘
       │              │              │
       ▼              ▼              ▼
  ┌─────────┐   ┌───────────┐  ┌──────────────┐
  │ ESC/POS │   │ Barcode   │  │ EFD / SDC    │
  │ printer │   │ scanner   │  │ (NBR, v2)    │
  │ + drawer│   │ or camera │  └──────────────┘
  └─────────┘   └───────────┘
```

The internet is never between the cashier and the sale. It carries sync, backups and back office.

## Level 2: containers

| Container | Tech | Responsibility | Notes |
|---|---|---|---|
| `core/` | Rust crate | Every decision: pricing and VAT math, in-memory replica and indices, snapshot and delta storage, outbox and sync engine, lease consumption, offline PIN and permission checks | Compiles to WASM, to an Android native library, and links into the server. One implementation of the money path |
| `apps/till-android` | Flutter, `flutter_rust_bridge` | Thin UI over the core; ESC/POS printing, drawer, camera scan, kiosk | 40 to 80 MB resident against 150 to 250 MB for a WebView |
| `apps/till-web` | Svelte 5, Vite, Workbox `injectManifest` | Same thin UI for desktop counters, demo and self-host evaluation | Runs the core as WASM **in a dedicated Web Worker**: OPFS sync access handles are worker-only, and holding `&mut Replica` across JS turns on the main thread is the classic wasm-bindgen panic |
| `apps/server` | Rust, Axum, `sqlx`, Postgres | Sync hub, back office API, tenancy, lease issue, repair queue; serves the admin SPA | Single static binary, so self-host is a small image plus Postgres. Bodies are postcard, not JSON: tills sync over prepaid mobile data |
| `apps/admin` | Svelte SPA | Catalogue, stock, reports, terminal health, repair queue | No SSR, no second runtime to deploy |
| Postgres | 16+ | All server state, append-only ledgers | Shared tables, `tenant_id` everywhere. RLS uses FORCE so the table owner is subject to it too, and every policy carries both USING and WITH CHECK, because USING alone silently refuses every insert. The app connects as a non-superuser role that cannot alter the schema |
| Backup sidecar | container + cron | `pg_dump` to volume and to R2 on the hosted tier | Restore documented and drilled in CI |
| Caddy | reverse proxy | TLS for self-host | Cloudflare fronts the hosted tier instead |

## Level 3: components inside `core/`

| Component | Responsibility |
|---|---|
| `domain` | Pricing, discounts, VAT, rounding, change, totals. Pure, no I/O, property-tested. Integer money and quantities enforced by types |
| `replica` | In-memory catalogue with barcode, code and token indices. 0.38 us lookups on a 12x throttled CPU, no I/O on the scan path |
| `storage` | Frame protocol: envelope, checksums, torn-write recovery, A/B snapshot slots, checkpoint policy. Backends are thin: `rusqlite` on Android, OPFS sync access handles in a Web Worker, `std::fs` and in-memory for tests |
| `storage::commit` | One atomic durable unit per sale: ticket, lease-after state, shift and cash movement, outbox entry. With an explicit flush barrier, because a receipt must not print before the sale is durable |
| `checkpoint` | Rewrites the packed snapshot off the input path. Never writes 20,000 rows individually |
| `sync` | Pull by cursor, push batches, lease renewal, backoff, protocol version negotiation. Persists a pulled batch before applying it, and advances the cursor only once that write is durable |
| `outbox` | Derived from the critical log, not stored beside it. Acknowledgement appends a watermark; the log is emptied only when nothing is outstanding, because deleting from the front means a rewrite that can lose the unacknowledged tail |
| `lease` | Holds the receipt-number block and epoch; consumed offline |
| `shift` | Terminal-scoped shift state, cash movements, X and Z totals |
| `auth` | Hashed PIN verification, permission snapshot with expiry, privileged-action log |

## Level 3: what lives in the UI layer

Rendering, input capture and hardware only. The scan buffer collects key events and calls the core
once per completed barcode. Printing renders a receipt the core produced. No business rules, no
duplicated arithmetic; if a UI needs to decide something, that decision belongs in `core/`.

## Data flows

1. **Sale.** UI sends intent to `core`; `domain` computes totals; the ticket is appended to `outbox`
   and durably stored; `lease` yields the receipt number; the UI prints. No network, no I/O on the
   lookup path.
2. **Drain.** `sync` pushes outbox batches to a single-transaction ingest endpoint; the server is
   idempotent on ULID and revalidates totals with the same `domain` code; on success the unsynced
   counter decrements. Permanent failures move to `repair_item` rather than blocking the queue.
3. **Pull.** `sync` requests changes after its cursor; the server returns rows plus tombstones;
   `replica` applies them in memory, `checkpoint` rewrites the snapshot off the input path.
4. **Stock.** Every sale, receipt, adjustment and count barrier appends to `stock_movement`.
   `on_hand` is materialised and rebuildable from the ledger with barrier semantics.
5. **Lease renewal.** Whenever online, the till tops its block up. A terminal that sells outside its
   lease is detected server-side and repaired, never rejected.
6. **Backup.** Nightly dump to volume, and to R2 on the hosted tier.

## Deployment

**Self-host:** `docker compose up` gives api, postgres, caddy and the backup sidecar. Single tenant,
billing compiled out behind an env flag.

**Hosted:** the same image, multi-tenant, on a VPS with managed Postgres to start. Cloudflare in
front for DNS, WAF, Access on the admin surface, and R2 for backups. Cloudflare Containers is a
later optimisation, not a v1 dependency.

## Decisions log

| Date | Decision | Reason |
|---|---|---|
| 2026-09-06 | Vite SPA, not Next.js, for the till | Deterministic precache; Next's hashed chunks make full precache fragile and a partial service worker update bricks cold start |
| 2026-09-06 | No UI component library on either target | Measured: once lookups are sub-microsecond, runtime and parse dominate. Vuetify-based POS Awesome hung on catalogue load in one run of three |
| 2026-09-06 | Catalogue in memory, IndexedDB for durability only | Measured 0.04 us vs 0.1 to 0.2 ms per lookup, 2,500x to 5,000x |
| 2026-09-06 | Packed snapshot plus delta log, never 20k row writes | Measured 9.6 ms snapshot hydrate vs 85.6 ms row load vs 1,501 ms row write, the last being 7 to 20 s on target hardware |
| 2026-09-06 | Admin is a Svelte SPA served by the server | No SSR and no second runtime to self-host |
| 2026-09-06 | Rust core compiled to WASM and native, superseding a TypeScript till | One implementation of the money and sync path; native RAM and boot on cheap Android while keeping a browser build |
| 2026-09-06 | Flutter for the Android till, superseding Capacitor | A WebView costs 150 to 250 MB on a 2 GB tablet and risks OS eviction, which forces the cold start the product exists to survive |
| 2026-09-06 | Axum server sharing the core crate, superseding Fastify and Drizzle | The server revalidates synced tickets with the identical arithmetic the till used, and ships as a single static binary for self-host |
| 2026-09-06 | Measured before choosing: language was not the bottleneck | On a 12x throttled CPU an in-memory lookup is 0.38 us against a 50 ms budget. Native was chosen for RAM, boot and hardware access, not throughput |
| 2026-09-06 | No Redis or queue in v1 | The till is the queue; extra containers are self-host support tickets |
| 2026-09-06 | Shared tables plus `tenant_id`, RLS as second belt | Schema-per-tenant is migration pain for a solo maintainer; db-per-tenant is a 2,000-shop answer |
| 2026-09-06 | Server-leased receipt-number blocks with epochs | Terminal-owned sequences duplicate numbers after a restore or storage wipe, and rejection arrives after the customer has the receipt |
| 2026-09-06 | v1 does not promise gapless numbering | Incompatible with offline multi-writer allocation; the EFD assigns the fiscal number in the v2 compliance layer |
| 2026-09-06 | Shifts are terminal-scoped | A shop-wide shift row is the one write conflict the append-only model cannot absorb |
| 2026-09-06 | Stock counts are ledger barriers | Ordering by client timestamp lets a late offline sale silently rewrite a completed count |
| 2026-09-06 | `branch_id` in the schema from day one | Backfilling a branch column across a live ledger is the worst migration available |
| 2026-09-06 | Storage trait stays synchronous, core runs in a Web Worker | Async in trait puts suspension points inside sale commit, so a scan arriving mid-await is a reentrancy bug; it is also not dyn-compatible, forcing three executors |
| 2026-09-06 | OPFS sync access handles, not IndexedDB | IndexedDB durability is a hint, Chrome defaults to relaxed, and the earlier 100 us measurement measured the timer not the disk. OPFS `flush()` and SQLite FULL are the only primitives with defensible semantics |
| 2026-09-06 | Transactional commit replaces the five-operation trait | Separate appends permit a crash between ticket and lease, producing a ghost receipt number or number reuse after reboot |
| 2026-09-06 | Two stores with separate lifecycles | Critical (tickets, tenders, outbox, lease, shift) and replica cache differ in truncation trigger, durability and loss semantics. One log serving both means a checkpoint silently deletes unsynced sales |
| 2026-09-06 | Protocol engine in the core, backends dumb | Framing, checksums, recovery and checkpoint are property-tested once against a fault-injecting mock, rather than reimplemented in Dart and in JS where tests cannot reach |
| 2026-09-06 | postcard on disk, wire types separate from domain types | postcard is positional: adding a field to `Item` turns every old snapshot into garbage. Disk needs backward compatibility, the sync wire needs forward compatibility too, and one struct serving both makes a wire change force a disk migration |
