# openpos feature spec, v1

Status: approved design, ready for implementation planning. Written 2026-09-06.
Supersedes nothing. Companion docs: [c4model.md](c4model.md) for architecture, [index.md](index.md).

## 1. What openpos is

An offline-first point of sale for small retail shops, built ground up. AGPL-3.0, whole codebase
public, free to self-host with `docker compose up`. Revenue comes from a managed cloud tier that
sells hosting convenience, not features: managed upgrades, backups, uptime, support. Single feature
set, billing behind an environment flag. This is the Postiz model, verified in that codebase:
AGPL-3.0 across the repo, `IS_GENERAL` env flag for branding rather than gating, one
`docker-compose.yaml` for self-hosters.

First market is Bangladesh general retail: grocery-type shops, one to five terminals, occasionally
multiple branches. The product is not Bangladesh-only, but the first compliance work will be.

## 2. The wedge

A hands-on evaluation of the existing open source field was completed on 2026-09-05
(`/Users/macmini/projects/pos-eval/docs/pos_evaluation_results.md`). Measured, not assumed:

| System | Sells during an outage | Syncs on reconnect | Cold start during an outage |
|---|---|---|---|
| Odoo 19 CE POS | yes, 2 sales, receipts printed | yes, automatic, numbering and stock exact | **no**, reload gives a browser error page |
| ERPNext + POS Awesome | yes, 5 sales | yes, all 5 within about a second | **no**, "POS app failed to start before the boot controller could run" |
| NexoPOS 6.2.2 | no | n/a | no service worker or IndexedDB at all |

Nobody can start a till during an outage. A shop that opens at 8am with the internet down cannot
sell. **openpos treats cold-start-offline as the headline requirement**: the till boots from cold,
authenticates, loads its catalogue and rings a sale with the network unplugged. Everything else in
this spec is negotiable; this is not.

Corollary requirement: the offline queue is visible to the cashier at all times (count of unsynced
tickets, last successful sync) and drains automatically without an operator pressing anything.
POS Awesome gets the visibility right and the automatic part wrong; Odoo the reverse.

## 3. Scope

**v1 covers** the till, sync and tenancy, catalogue and stock and purchasing, and cash and shifts
and roles. A real shop can run its whole selling and stock life on it.

**v1 explicitly excludes** Bangladesh fiscal compliance (Mushak 6.3, EFD/SDC devices), analytics
beyond basic reports, loyalty and coupons, restaurant or table service, e-commerce sync, and
accounting beyond what a POS needs. Each gets its own spec later.

Feature tags below: **v1** in the first release, **v2** next, **later** acknowledged and deferred.

## 4. Feature inventory

### 4.1 Selling

| Feature | Tag | Note |
|---|---|---|
| Barcode scan to cart, hardware scanner as keyboard wedge | v1 | EAN-13 and Code128 minimum |
| Camera scanning on the Android shell | v1 | cheap tablets often have no scanner |
| Search by item code, name, and local-language name | v1 | must work fully offline, indexed locally |
| Quantity edit, including decimal quantities for loose goods | v1 | quantities stored as integer milli-units |
| Line discount, amount or percentage | v1 | subject to the cashier ceiling |
| Whole-ticket discount | v1 | same ceiling |
| Discount ceiling per role | v1 | prevents a hired cashier discounting freely |
| Price override at the till | v1 | permission-gated, logged |
| Sell below cost guard | v2 | warn or block, configurable |
| Hold and resume a ticket | v1 | multiple parked tickets per terminal |
| Line notes and item rename on a line | v2 | |
| Weighed items and scale barcodes | v2 | embedded-weight barcode parsing |
| Item quick-add for an unknown barcode mid-outage | v1 | see 7.4, this is a cold-start consequence |
| Fast counter mode, minimal keystrokes | v2 | |
| Returns against a ticket | v1 | creates a reversing document, never a delete |
| Returns without the original ticket | v2 | permission-gated |
| Return validity window | v2 | configurable days |

### 4.2 Payments and tenders

| Feature | Tag | Note |
|---|---|---|
| Cash tender with change calculation | v1 | |
| Multiple tenders on one ticket | v1 | cash plus wallet is the common Bangladesh case |
| Tender types as an extensible enum | v1 | bKash and Nagad will be the first request; the seam must exist in v1 |
| Manual wallet tender (record a bKash or Nagad reference) | v1 | no API integration, just a recorded reference |
| bKash and Nagad API integration | v2 | needs merchant accounts, callbacks and a public endpoint; hosted tier first |
| Card terminal integrations | later | no credible open path in this market yet |
| Rounding rules | v2 | |
| Credit sale, pay later, customer balance | v2 | |
| Multi-currency tendering | later | not a Bangladesh retail need |

### 4.3 Customers

| Feature | Tag | Note |
|---|---|---|
| Walk-in default customer | v1 | |
| Assign a customer to a ticket | v1 | |
| Create a customer at the till, offline | v1 | syncs as a normal entity with a ULID |
| Phone number as the lookup key | v1 | how Bangladeshi shops actually identify customers |
| Tax ID / BIN field on the customer, printed on the receipt | v1 | costs nothing now, required by Mushak 6.3 later |
| Customer balance and credit limit | v2 | pairs with credit sale |
| Customer-specific price lists | v2 | |

### 4.4 Catalogue and pricing

| Feature | Tag | Note |
|---|---|---|
| Items with code, names in two scripts, unit, barcodes (many per item) | v1 | Bangla and English names both searchable |
| Item categories | v1 | |
| Cost and selling price | v1 | money as integer minor units throughout |
| Price lists, per branch | v2 | |
| Time-bound promotional prices | v2 | |
| Item images | v2 | excluded from the till replica on purpose, see 7.3 |
| Variants and templates | later | |
| Batch and expiry tracking | v2 | matters for a grocery, not for the first release |
| Serial numbers | later | |

### 4.5 Stock

| Feature | Tag | Note |
|---|---|---|
| Stock as an append-only movement ledger | v1 | on-hand is derived, never the source of truth |
| Stock decrement on sale, including offline sales | v1 | |
| Live on-hand on the till, from the local replica | v1 | shown per item like POS Awesome does |
| Stock take with barrier semantics | v1 | a count is an assertion, not a delta, see 7.8 |
| Block or warn on selling beyond on-hand | v1 | configurable per shop |
| Adjustments with a reason code | v1 | breakage, theft, expiry |
| Transfers between branches | v2 | |
| Valuation, weighted average | v2 | needed before profit reporting is honest |
| Low stock alerts and reorder points | v2 | |

### 4.6 Purchasing

| Feature | Tag | Note |
|---|---|---|
| Suppliers | v1 | |
| Goods receipt against a supplier, at the counter | v1 | shopkeeper receives stock where they stand |
| Purchase price capture and cost update | v1 | |
| Purchase orders | v2 | small shops buy without a PO; do not force one |
| Supplier payments and balances | v2 | |
| Purchase returns | v2 | |

### 4.7 Cash, shifts and roles

| Feature | Tag | Note |
|---|---|---|
| Open a shift with a counted opening float | v1 | shift is terminal-scoped, see 7.6 |
| Close a shift with a counted total and variance | v1 | |
| Blind close, cashier cannot see expected | v2 | |
| Cash in and cash out during a shift, with a reason | v1 | |
| Petty expense paid from the drawer | v2 | |
| X report during a shift, Z report at close | v1 | |
| Shop-level day report aggregating terminals | v1 | aggregation, not a shared mutable row |
| Roles: owner, manager, cashier | v1 | |
| Hashed cashier PIN, verified offline | v1 | offline auth is part of cold start, see 7.7 |
| Permission snapshot in the replica with an expiry | v1 | a revoked cashier must stop working within a bounded window |
| Supervisor override with a PIN for gated actions | v1 | discounts above ceiling, price override, returns |

### 4.8 Offline and sync

| Feature | Tag | Note |
|---|---|---|
| Cold start with no network: boot, log in, sell | v1 | the wedge, asserted in CI |
| Full catalogue replica in IndexedDB | v1 | images excluded |
| Write-ahead ticket log, append-only | v1 | |
| Automatic drain on reconnect, no operator action | v1 | |
| Visible queue: unsynced count, last sync time, per-ticket state | v1 | |
| Manual "sync now" as a fallback, not the primary path | v1 | |
| Survive a page reload, a tab close, and an app restart mid-outage | v1 | measured failure mode in POS Awesome |
| Storage persistence guaranteed on Android | v1 | native shell, not `navigator.storage.persist()` alone |
| Delta sync with a per-tenant monotonic cursor | v1 | not `updated_at`, see 7.2 |
| Tombstones for deletes | v1 | |
| Local schema migrations for IndexedDB | v1 | prevents old tills bricking after a deploy |
| Sync protocol versioning | v1 | a till offline for two weeks must sync into a newer API |
| Conflict and repair queue in the back office | v1 | duplicate receipt numbers, late stock movements |

### 4.9 Printing and hardware

| Feature | Tag | Note |
|---|---|---|
| ESC/POS thermal receipt printing, 58 mm and 80 mm | v1 | via the Android shell over Bluetooth and USB |
| Cash drawer kick | v1 | through the printer |
| Bangla script on the receipt | v1 | requires a font-rendered raster path on most cheap printers |
| Receipt template with shop header, footer, BIN fields | v1 | |
| Reprint with an audit record | v1 | prevents unverifiable cash disputes |
| Print from the desktop browser build | v2 | local print agent |
| Barcode label printing | v2 | |
| Customer-facing display | later | |

### 4.10 Tax

| Feature | Tag | Note |
|---|---|---|
| Multiple tax rates and an exempt class | v1 | Bangladesh standard rate is 15 percent; rates are configuration, not code |
| Tax inclusive or exclusive pricing per shop | v1 | |
| VAT stored per line in basis points | v1 | |
| Tax totals on the receipt and in the Z report | v1 | |
| Mushak 6.3 layout, EFD/SDC bridge, fiscal numbering | v2 | separate spec, gated on verifying NBR rules against a primary source and obtaining device access |

### 4.11 Back office

| Feature | Tag | Note |
|---|---|---|
| Item, customer and supplier management | v1 | |
| Stock on hand and movement history | v1 | |
| Sales list with drill-down to a ticket | v1 | |
| Day and date-range sales report, by terminal and by branch | v1 | |
| Top items and slow movers | v2 | |
| Profit report | v2 | honest only after valuation lands |
| Terminal health: last seen, unsynced count, app version | v1 | this is the support-load ceiling for a solo maintainer |
| Repair queue for sync conflicts | v1 | |
| Tenant export and import | v1 | see 7.8 |

### 4.12 Platform

| Feature | Tag | Note |
|---|---|---|
| Multi-tenant: tenant, branch, terminal on every relevant row | v1 | `branch_id` from day one even with a single-branch UI |
| Self-host: `docker compose up` with api, postgres, caddy | v1 | |
| Backup sidecar with a documented and tested restore | v1 | a self-hoster losing a disk blames the project publicly |
| Hosted tier: same image, billing behind an env flag | v1 | |
| Terminal enrollment and device identity | v1 | prevents cloned or restored terminals corrupting numbering |
| Service worker update safety | v1 | never activate a new shell until its precache and DB migrations are ready |
| i18n, English and Bangla | v1 | |
| Observability: sync queue state, failed events, terminal health | v1 | |

## 5. Architecture decisions

Each decision names the failure it prevents. Reviewed adversarially by two independent reviews on
2026-09-06; both are archived at `/Users/macmini/projects/codex/openpos_architecture_review.txt`.

**5.1 Till is a Vite SPA with an explicit Workbox precache, wrapped in Capacitor for Android.**
Not Next.js. Hashed RSC chunks and route-level code splitting make "precache one hundred percent of
the till" fragile, and a half-updated service worker is a till that cannot boot during the next
outage, which is the exact failure the product exists to prevent. The precache manifest is asserted
in CI. The Android wrapper buys three things the web platform cannot give on Android: guaranteed
storage persistence rather than evictable IndexedDB, ESC/POS printing over Bluetooth and USB, and
kiosk mode. A browser build stays available for desktop counters.

**5.2 Server is a plain Node API over Postgres, no queue in v1.**
**Fastify with Drizzle.** NestJS was considered and rejected as ceremony this product does not yet
need; the decision is low stakes and reversible, but the spec picks one so the plan does not have to.
No Redis and no BullMQ: sync is request and response, the till is the queue, and every extra
container is a support ticket for a self-hoster on a cheap VPS. Prisma is rejected because row-level
security needs `SET LOCAL app.tenant_id` inside a per-request transaction, which Prisma's pooling
makes awkward; Drizzle gives typed SQL without fighting the connection lifecycle.

**5.3 Shared domain package runs identically on both sides.**
Pricing, discount, tax and rounding math lives in `packages/domain`, dependency-free, imported by
both till and server. If the offline total and the server total ever disagree by one poisha,
reconciliation becomes unfalsifiable. Same code, same result, property-tested on both.

**5.4 The API is never on the critical path of a sale.**
A sale completes locally, always. The server is a sync hub and a back office.

**5.5 Tenancy: one Postgres, shared tables, `tenant_id` on every tenant-owned row.**
Schema-per-tenant is the wrong answer for a solo maintainer: two hundred schemas times every
migration, with half-failed runs leaving drift. Database-per-tenant is the two-thousand-shop answer.
Row-level security is a second belt, not the first: the app connects as a non-owner, non-BYPASSRLS
role, because table owners bypass RLS by default and most deployments never notice. The first belt
is a query layer that refuses to emit SQL without a tenant scope. What actually breaks first is not
scale: at ten shops it is an RLS misconfiguration or a missing composite index with `tenant_id`
leading; at two hundred it is per-shop restore and support debugging.

**5.6 Money as integer minor units, quantities as integer milli-units, VAT in basis points.**
No floats anywhere in the money path. An auditor re-adds these by hand.

## 6. Data model, core tables

Sketch, not final DDL. Every table below carries `tenant_id`, and everything transactional also
carries `branch_id` and `terminal_id`.

```
tenant, branch, terminal, app_user, role_assignment
item, item_barcode, item_price, tax_class, category
customer, supplier
ticket            -- the sale, immutable once closed
ticket_line       -- sku, qty_milli, unit_price_minor, vat_rate_bp, discount_minor
tender            -- one row per tender on a ticket, type + amount_minor + reference
stock_movement    -- append-only: sale, receipt, adjustment, transfer, count_barrier
stock_count       -- header for a stock take, becomes a barrier in the ledger
on_hand           -- materialised, rebuildable from stock_movement at any time
shift             -- terminal-scoped, opening float, closing count, variance
cash_movement     -- in, out, reason, actor
receipt_number_lease  -- terminal, block start, block end, epoch, issued_at
sync_cursor       -- per-tenant monotonic sequence, the replication clock
outbox            -- server-side change feed the till pulls from
repair_item       -- detected conflicts awaiting a human decision
```

`ticket.id` is a ULID minted on the terminal. `ticket.receipt_no` comes from a lease. The two are
independent on purpose: identity never waits for the network, presentation does.

## 7. Sync protocol and the hard cases

**7.1 Identity and idempotency.** Every entity created on a terminal gets a ULID at creation.
Upload is at-least-once and idempotent: replaying a ticket is a no-op. The server never invents
identity for terminal-created records.

**7.2 Replication cursor.** The till pulls changes since a per-tenant monotonic server sequence, not
`updated_at`. Wall clocks skew and ties are unresolvable; a sequence is total and cheap. Deletes
propagate as tombstones so a deleted item disappears from the replica instead of lingering.

**7.3 What the till replicates.** All sellable items with barcodes, prices, tax classes, categories,
customers, the permission snapshot, the open shift, and on-hand hints. Twenty thousand SKUs at a few
hundred bytes each is single-digit megabytes and hydrates in seconds. Images are excluded: they are
where a few megabytes becomes gigabytes and eviction risk returns.

**7.4 Unknown barcode during an outage.** An item created at the back office mid-outage does not
exist on the till. The till offers quick-add: scan, name, price, sell. It syncs as a provisional item
the back office must confirm. Without this, the cold-start promise still loses the sale.

**7.5 Receipt numbering.** Terminal-owned gapless sequences were rejected: a terminal restored from a
backup or with cleared storage re-uses numbers, prints a receipt, and the customer leaves before any
server can object. Instead the server leases signed blocks per terminal, for example
`T1-000100..000599`, with an epoch. The till renews opportunistically whenever it is online and
consumes the block offline. If a restored terminal sells outside its lease, the server does not
reject: it accepts the ticket under its ULID, flags the number collision, and routes it to the repair
queue for reassignment. Detect and repair, never reject after the fact.

**v1 does not promise gapless numbering.** Never-skipped-and-never-duplicated is incompatible with
offline multi-writer allocation, and writing that promise into v1 is a promise to be un-made in front
of an auditor. When Mushak 6.3 arrives, the EFD or SDC device assigns the fiscal number at its own
layer, which is where the gapless guarantee properly belongs.

**7.6 Shifts are terminal-scoped.** A shop-wide open shift is a shared mutable row, and two offline
terminals closing it is exactly the conflict the append-only model exists to avoid. Each terminal or
drawer owns its shift; the shop-level Z report is an aggregation over terminals.

**7.7 Offline authentication.** Cold start includes logging in. Cashier PINs are hashed into the
replica alongside a permission snapshot carrying an expiry. Offline enforcement is client-side by
definition, so every offline privileged action is logged for later review, and the snapshot expiry
bounds how long a revoked cashier keeps working.

**7.8 Stock takes and late-arriving sales.** A count is an absolute assertion about a shelf; a sale
is a delta. Ordering movements by client timestamp corrupts counts. A confirmed count writes a
barrier into the ledger, and on-hand equals the last barrier plus movements whose business time
falls after it, so a sale that arrives late but was rung before the count is correctly absorbed. Two
supports are required: per-terminal clock skew recorded at every sync, storing both device time and
server receipt time; and an operational guard that blocks confirming a stock take while any terminal
has unsynced tickets, naming the terminal and the count. The algorithm alone cannot know whether the
counted shelf included a buffered sale. The guard is what saves the number.

**7.9 Protocol versioning.** Payloads are versioned. A till that has been offline for two weeks must
sync into a newer API. The server supports the previous protocol version for at least one release.

## 8. Error handling

- **A sale can never fail because of the network.** Any server error during sync leaves the ticket in
  the queue and surfaces in the counter; it does not block the next sale.
- **Poison messages.** A ticket the server rejects for a permanent reason (malformed, unknown item
  after tombstone) moves to the repair queue after bounded retries rather than blocking the queue
  head. Queue drain is per-ticket, not strictly ordered, except where a ticket references another.
- **Service worker updates.** A new shell activates only when its full precache and any IndexedDB
  migration have completed. Until then the old shell keeps serving. A failed update is a no-op, never
  a bricked till.
- **Storage pressure.** The Android shell claims persistent storage at enrollment. If persistence is
  refused, the till warns the owner rather than pretending to be safe.
- **Clock nonsense.** Device clocks are recorded, not trusted. Business ordering uses the server
  sequence and barrier semantics.

## 9. Testing strategy

- **The wedge test runs in CI from week one.** Playwright: seed a shop, kill the server, hard-reload
  the till, ring a sale from cold, restore the server, assert every ticket drains, numbering is
  intact and on-hand is exact. Both leading products regressed on cold start; that only stays fixed
  if a machine checks it on every commit.
- **Domain math is property-tested** on both runtimes: discounts, VAT, rounding and change never
  disagree between till and server.
- **Ledger rebuild test:** on-hand recomputed from the movement ledger equals the materialised table
  for a randomised movement history including late arrivals and barriers.
- **Migration tests** for both Postgres and the IndexedDB schema, including an old till syncing into
  a new server.
- **Restore drill in CI** for the self-host backup sidecar: dump, drop, restore, assert row counts.

## 10. Non-goals for v1

Restaurant and table service. E-commerce or marketplace sync. Loyalty, coupons and gift cards. Full
accounting. Payroll. Multi-currency. Franchise or reseller hierarchies. Card terminal integrations.
Anything requiring an NBR device before the rules are verified.

## 11. Open questions, to resolve before the affected milestone

1. **NBR rules must be verified against a primary source** before any fiscal work. Everything
   currently known came from vendor blogs and is marked unverified in the evaluation workspace.
2. **Which thermal printers to certify.** Pick two or three cheap models common in Dhaka and test
   Bangla rendering on each; receipt output on most cheap ESC/POS units needs a raster path.
3. **Android distribution.** Play Store listing or sideload for shops without Play services.
4. **DCO or CLA before the first external contribution.** Without it the project can never
   relicense or dual-license, and consent cannot be retrofitted from drive-by contributors.
5. **Hosting substrate for the paid tier.** Cloudflare Containers is unproven for this workload; a
   boring VPS with managed Postgres is the lower-risk start, with Cloudflare in front for DNS, WAF,
   Access and R2 backups.
