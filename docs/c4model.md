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
| `apps/till` | Vite SPA, Workbox `injectManifest`, IndexedDB (Dexie) | The entire sale: catalogue, cart, tenders, receipt, shift, offline queue | Precache manifest asserted in CI. Never depends on the server to complete a sale |
| Android shell | Capacitor | Storage persistence, ESC/POS over Bluetooth and USB, drawer kick, kiosk mode | The web platform cannot print to ESC/POS on Android; this is why the shell exists |
| `apps/api` | Node, Fastify, Drizzle, Postgres | Sync hub, back office API, tenancy, leases, repair queue | No Redis and no queue in v1 |
| `apps/admin` | Next.js | Back office web: catalogue, stock, reports, terminal health, repair queue | May use SSR freely; it has no offline requirement |
| `packages/domain` | TypeScript, dependency-free | Pricing, discounts, VAT, rounding, change | Imported by till and api; identical results on both sides or reconciliation is unfalsifiable |
| `packages/sync` | TypeScript | Protocol types, cursor logic, envelope versioning | Shared by till and api |
| Postgres | 16+ | All server state, append-only ledgers | Shared tables, `tenant_id` everywhere, RLS as a second belt |
| Backup sidecar | container + cron | `pg_dump` to local volume and to R2 on the hosted tier | Restore is documented and drilled in CI |
| Caddy | reverse proxy | TLS for self-host | Cloudflare fronts the hosted tier instead |

## Level 3: components inside the till

| Component | Responsibility |
|---|---|
| `replica` | IndexedDB store of items, barcodes, prices, tax classes, customers, permission snapshot; local schema migrations |
| `catalogue-index` | Local search and barcode lookup; the real performance risk, not storage size |
| `cart` | Ticket assembly, calls `packages/domain` for all money math |
| `tender` | Cash and wallet tenders, change, extensible tender types |
| `outbox` | Append-only write-ahead log of tickets and terminal-created entities; drives the visible counter |
| `sync-agent` | Pull by cursor, push outbox, lease renewal, backoff, protocol version negotiation |
| `lease` | Holds the receipt-number block and epoch; consumed offline |
| `shift` | Terminal-scoped open shift, cash movements, X and Z |
| `printer` | ESC/POS rendering including a raster path for Bangla, drawer kick; native bridge on Android |
| `auth-offline` | Hashed PIN verification, permission snapshot with expiry, privileged-action log |

## Data flows

1. **Sale.** Cart to `packages/domain` for totals, ticket written to `outbox` and IndexedDB,
   receipt number consumed from `lease`, receipt printed. No network involved.
2. **Drain.** `sync-agent` pushes outbox batches; server is idempotent on ULID; on success the
   counter decrements. Permanent failures move to `repair_item` rather than blocking the queue.
3. **Pull.** `sync-agent` requests changes after its cursor; server returns rows plus tombstones from
   the outbox feed; replica applies them and advances the cursor.
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
| 2026-09-06 | Capacitor Android shell | Storage persistence and ESC/POS printing are unavailable to a browser on Android |
| 2026-09-06 | No Redis or queue in v1 | The till is the queue; extra containers are self-host support tickets |
| 2026-09-06 | Fastify and Drizzle, not NestJS or Prisma | NestJS is ceremony this product does not need yet; RLS needs `SET LOCAL` per transaction, which Prisma's pooling fights |
| 2026-09-06 | Shared tables plus `tenant_id`, RLS as second belt | Schema-per-tenant is migration pain for a solo maintainer; db-per-tenant is a 2,000-shop answer |
| 2026-09-06 | Server-leased receipt-number blocks with epochs | Terminal-owned sequences duplicate numbers after a restore or storage wipe, and rejection arrives after the customer has the receipt |
| 2026-09-06 | v1 does not promise gapless numbering | Incompatible with offline multi-writer allocation; the EFD assigns the fiscal number in the v2 compliance layer |
| 2026-09-06 | Shifts are terminal-scoped | A shop-wide shift row is the one write conflict the append-only model cannot absorb |
| 2026-09-06 | Stock counts are ledger barriers | Ordering by client timestamp lets a late offline sale silently rewrite a completed count |
| 2026-09-06 | `branch_id` in the schema from day one | Backfilling a branch column across a live ledger is the worst migration available |
