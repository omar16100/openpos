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
| `apps/till-android` | Flutter over `openpos-ffi`, a plain C ABI | Thin UI over the core; ESC/POS printing, drawer, camera scan, kiosk | 40 to 80 MB resident against 150 to 250 MB for a WebView. Dart FFI calls the C ABI directly: no `flutter_rust_bridge`, no code generator in the build |
| `ffi/` | Rust, C ABI, cdylib and staticlib | Four exported functions, one of which carries every operation as a JSON command | The only crate in the workspace allowed to write unsafe, and the reason the rest can forbid it |
| `apps/till-web` | Svelte 5, Vite, Workbox `injectManifest` | Same thin UI for desktop counters, demo and self-host evaluation | Runs the core as WASM **in a dedicated Web Worker**: OPFS sync access handles are worker-only, and holding `&mut Replica` across JS turns on the main thread is the classic wasm-bindgen panic |
| `apps/server` | Rust, Axum, `sqlx`, Postgres | Sync hub, back office API, tenancy, lease issue, repair queue; serves the admin SPA | Single static binary, so self-host is a small image plus Postgres. Bodies are postcard, not JSON: tills sync over prepaid mobile data |
| `apps/admin` | Svelte SPA | Catalogue, stock, reports, terminal health, repair queue | No SSR, no second runtime to deploy |
| Postgres | 16+ | All server state, append-only ledgers | Shared tables, `tenant_id` everywhere. RLS uses FORCE so the table owner is subject to it too, and every policy carries both USING and WITH CHECK, because USING alone silently refuses every insert. The app connects as a non-superuser role that cannot alter the schema |
| Backup sidecar | container + cron | `pg_dump` to volume and to R2 on the hosted tier | Restore documented and drilled in CI |
| Caddy | reverse proxy | TLS for self-host | Cloudflare fronts the hosted tier instead. Either one requires `OPENPOS_TRUSTED_PROXY_HOPS=1`, or enrolment rate limiting sees every client in the world as the proxy and throttles them as one |

## Level 3: components inside `core/`

| Component | Responsibility |
|---|---|
| `receipt::escpos` | The same lines as bytes for a thermal printer. Text outside printable ASCII is marked and the line reported, because no standard ESC/POS codepage carries Bengali and sending the bytes anyway prints Latin mojibake in a customer's hand |
| `receipt` | The receipt laid out for a printer: lines of text and an emphasis flag, at a given character width. Rendered here because a browser receipt and a tablet receipt that differ are two documents describing one sale, and a dispute is settled against paper |
| `domain` | Pricing, discounts, VAT, rounding, change, totals. Pure, no I/O, property-tested. Integer money and quantities enforced by types |
| `replica` | In-memory catalogue with barcode, code and token indices. 0.38 us lookups on a 12x throttled CPU, no I/O on the scan path |
| `storage` | Frame protocol: envelope, checksums, torn-write recovery, A/B snapshot slots, checkpoint policy. Backends are thin: `rusqlite` on Android, OPFS sync access handles in a Web Worker, `std::fs` and in-memory for tests |
| `storage::commit` | One atomic durable unit per sale: ticket, lease-after state, stock movements, outbox entry. With an explicit flush barrier, because a receipt must not print before the sale is durable. Shift and cash movements are separate frames, not part of the sale payload |
| `storage::terminal_state` | The terminal's standing state (leased blocks, parked baskets) in an A/B blob slot outside the critical log, because that log is emptied when the server confirms everything in it |
| `checkpoint` | Rewrites the packed snapshot off the input path. Never writes 20,000 rows individually |
| `sync::driver` | Decides what to sync next and how long to wait after a failure. The platform performs the request; this says whether there should be one. Retry policy written twice is retry policy that differs, and the difference shows up as a shop whose sales sat on a tablet for a day |
| `sync` | Pull by cursor, push batches, lease renewal, backoff, protocol version negotiation. Persists a pulled batch before applying it, and advances the cursor only once that write is durable |
| `outbox` | Derived from the critical log, not stored beside it. Acknowledgement appends a watermark; the log is emptied only when nothing is outstanding, because deleting from the front means a rewrite that can lose the unacknowledged tail |
| `lease` | Holds the receipt-number block and epoch; consumed offline |
| `shift` | Terminal-scoped shift state, cash movements, X and Z totals with the declared-against-expected variance |
| `auth` (server) | Two credential roles: a till rings sales and syncs, an owner does that and the back office. Distinct from who is signed in at the till, which the core answers: a tablet left on a counter is a risk whoever is logged in |
| `auth` | PIN verification against PBKDF2 credentials held on the device, per-operator lockout on repeated guesses, single-use supervisor authorisation with an expiry, and an audit entry naming who allowed each privileged action. Credentials live in the standing-state blob, because signing in has to work with the internet down |

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
| 2026-09-06 | A loss is its own movement kind, not a count and not a sale | The question at the end of a bad month is which of the three it was. Folding a loss into a count makes every one look like a counting mistake and hides the pattern that says otherwise; folding it into a sale puts goods nobody paid for into the day's takings |
| 2026-09-06 | A correction demands a reason, enforced by the column | An unexplained write-off is indistinguishable from theft when the variance is read a month later, which is why a cash movement demands one too |
| 2026-09-06 | Only an owner may write stock off | A loss a cashier can record without anybody's knowledge is not a loss anybody investigates |
| 2026-09-06 | A movement carries its own occurrence and arrival times | They used to be read by joining back to the sale, which assumed every movement had a sale behind it. A goods receipt does not, and a barrier has to place both alike |
| 2026-09-06 | Unit cost is kept per delivery, not only on the item | The price a shop paid last Tuesday is what a margin is measured against; the item's standing cost is only the most recent guess at it |
| 2026-09-06 | A count keeps two times: when it was taken, and when it landed | The device clock decides which sales the count should already reflect. Server arrival decides which arrived too late to have been included. One time alone cannot tell those apart |
| 2026-09-06 | A sale rung before a count but arriving after it is excluded from on-hand and raised | Applying it decrements goods the counter may already have seen were gone; ignoring it silently loses a real sale. Neither is detectable afterwards, so the figure is carried separately and shown to a person |
| 2026-09-06 | An uncounted item reports a running total and says so | A figure resting on no count is a different kind of number, and a shop is entitled to know which it is looking at |
| 2026-09-06 | `branch_id` in the schema from day one | Backfilling a branch column across a live ledger is the worst migration available |
| 2026-09-06 | Storage trait stays synchronous, core runs in a Web Worker | Async in trait puts suspension points inside sale commit, so a scan arriving mid-await is a reentrancy bug; it is also not dyn-compatible, forcing three executors |
| 2026-09-06 | OPFS sync access handles, not IndexedDB | IndexedDB durability is a hint, Chrome defaults to relaxed, and the earlier 100 us measurement measured the timer not the disk. OPFS `flush()` and SQLite FULL are the only primitives with defensible semantics |
| 2026-09-06 | Transactional commit replaces the five-operation trait | Separate appends permit a crash between ticket and lease, producing a ghost receipt number or number reuse after reboot |
| 2026-09-06 | Two stores with separate lifecycles | Critical (tickets, tenders, outbox, lease, shift) and replica cache differ in truncation trigger, durability and loss semantics. One log serving both means a checkpoint silently deletes unsynced sales |
| 2026-09-06 | Protocol engine in the core, backends dumb | Framing, checksums, recovery and checkpoint are property-tested once against a fault-injecting mock, rather than reimplemented in Dart and in JS where tests cannot reach |
| 2026-09-06 | Terminals authenticate with a bearer token; identity comes from the credential, never the body | Before this, a request stated which shop it was and the server believed it, so a guessed pair of uuids could push sales or read a price list |
| 2026-09-06 | Token hashes stored with SHA-256, not argon2 | These are 256 random bits the server generates, so there is nothing to guess; a deliberately slow hash would only add latency to every request a shop makes |
| 2026-09-06 | The token table is the one exception to row-level security | It is what establishes which tenant a request belongs to, so it must be readable before the answer is known. It holds hashes and identifiers only |
| 2026-09-06 | Unprintable text is marked and reported, never sent and hoped for | An ESC/POS printer renders from a codepage in its firmware and none of them carries Bengali. Sending the bytes prints Latin letters and box drawing on a customer's receipt, and nobody finds out until a shopkeeper does |
| 2026-09-06 | Emphasis is switched only when it changes, and always turned off before a job ends | A printer keeps the setting across jobs, so a receipt left bold makes the next customer's bold too |
| 2026-09-06 | Scanning and looking up meet before the rules, not after | Both find an item and then hand it to one function that decides whether it may be sold. A second way onto a ticket that forgot the withdrawn check would be a way to sell what the shop has withdrawn |
| 2026-09-06 | The back office has its own module under `http/` | `http.rs` had reached 3,384 lines and the half still growing was the owner's half. Split by who the caller is rather than by verb, so the rule that every route here asks for an owner is a property of the file |
| 2026-09-06 | Failure paths are reached with a client, not a test endpoint | The repair queue, the duplicate-receipt check and the totals check only appear when something has gone wrong, and a browser cannot ring one sale twice under one number. `server/examples/restored_till.rs` behaves like a restored device using the public API. An endpoint for this would be a back door that ships |
| 2026-09-06 | The demo seeds whatever store is configured, behind `OPENPOS_DEMO` | Memory-only meant nothing that has to outlive a restart could be checked against the demo, which is most of what this product claims. Behind a flag, because demo data in a shop's real database is a catalogue nobody ordered and a person nobody hired |
| 2026-09-06 | The demo issues two codes, for two terminals | Two apps on one origin keep their stores in directories named for their terminal, so one code made the back office and the till fight over the same files |
| 2026-09-06 | Takings are summed from the sale headers | The total and the time are columns on the sale, so a day's figure needs no ticket decoded. Grouped in the database rather than pulled and summed in the server: a busy day is thousands of rows and the answer is one line per till |
| 2026-09-06 | Stock is asked for separately from the catalogue | A sale is not a catalogue change and must not bump the catalogue cursor, or every till re-pulls every item whenever anything sells. So the figure on an item record never moves, and what the shop holds is its own question |
| 2026-09-07 | A counted drawer is pushed, and held where truncation cannot reach it | The point of counting a drawer is that somebody who was not at the till reconciles it. The count lives in the standing state rather than the critical log, because the log is emptied when its sales are acknowledged and a counted drawer that went with it is a record nobody can reconstruct |
| 2026-09-07 | A counted drawer names whoever counted it, by copy | The signed-in operator is taken before anything moves, and their name is stored beside their id rather than joined at read time. Somebody who has since left the shop, or been renamed, is still the person that variance belongs to |
| 2026-09-07 | An upgrade is tested as one device, not three formats | Each legacy path had its own passing test and none described what a shop actually holds: a standing state, a snapshot and an unsent sale, each written by a build that predates a different field. That combination is where the bug lands |
| 2026-09-06 | A schema label is checked against the payload it labels | Writing the wrong number and reading the wrong constant cancel out exactly, and go on cancelling until one is corrected. `core/tests/schema_labels.rs` drives the real writers and reads the labels back, so neither half can drift alone |
| 2026-09-06 | The schema a blob was written under travels with its bytes | The reader was passing this build's constant, so a device upgrading would have decoded the previous build's standing state as the current version and lost its leases, its parked sales and its credential. Found by bumping the terminal schema for the first time |
| 2026-09-06 | Replacing a PIN is its own route, carrying nothing else | Amending somebody carries no credential; this carries a credential and nothing else. Two acts, two shapes, and neither can do the other by leaving a field out. The key is derived on the owner's device, so the digits never reach the server and a forgotten PIN can only be replaced |
| 2026-09-06 | Changing a person, except their PIN, is one route | The upsert takes the whole person including the derived key, and a PIN is hashed where it is set and never travels. `AmendedOperator` has no PIN field at all, so it is the type rather than a comment that stops one being cleared from here. Renaming and suspending are the same act from the server's side, so they are one route rather than two places to forget the owner check |
| 2026-09-06 | An override is the price; a discount is money off one | Tax follows an override down even on an item taxed on its listed price, and does not follow a discount down. The two are different acts, and the arithmetic already treated them so: this is the reading, written down after a test expected the other |
| 2026-09-06 | A retired item cannot be sold and can still be refunded | The flag was honoured by search and ignored by the lookup that takes money. Refusing the refund too would be worse: the shop sold it last week and the customer is holding it |
| 2026-09-06 | The back office reads the catalogue from its own replica | It syncs the same catalogue a till does, so an owner can see and correct prices with the line down. A search that needs the network is a search that fails in the shop it is for |
| 2026-09-06 | One worker for every app, in `apps/shared` | The till's and the back office's were the same file bar one branch, and drifted twice in a day. Each app keeps a ten-line entry, because the bundler resolves the wasm and the worker URL against the file that writes them |
| 2026-09-06 | The platform reports the status, the core decides what it means | A refused credential and a shop with no signal are the same failure to the code that does the HTTP, and only one is worth retrying. The decision belongs where the protocol is |
| 2026-09-06 | A failed request carries the view back with it | A request that failed still changes what the till knows, and a refused credential is exactly that. A screen handed only a string cannot show it, which is how a device went on looking enrolled |
| 2026-09-06 | An enrolment code names the till it is for | Minting a new id on every code meant a recovering device got an empty ledger and its unsent sales were stranded. The back office lists the tills and issues a code for one of them |
| 2026-09-06 | A command carries what a screen has, not what the core wants | A percentage arriving as basis points meant the JavaScript path and the typed path validated in different places, and the screen's discount button answered "missing field `discount_bp`" while every test passed |
| 2026-09-06 | The back office does not speak the protocol either | Requests are built and replies read by the same Rust the till uses. A second implementation is one that drifts, and the one used least drifts furthest |
| 2026-09-06 | A device enrols before it opens a store | The code decides which terminal the device is, and a store has to be opened as somebody. Opening one as a guess made a second device present a credential for one terminal and a request body for another |
| 2026-09-06 | Each device's store lives in a directory named for its terminal | Two apps on one origin share an OPFS root. The till and the back office were opening the same files as different terminals, which the journal's owner check refused, correctly |
| 2026-09-06 | A PIN is hashed on the owner's device, never sent | The salt, rounds and derived key are computed with the same code the till verifies with, so the operator table is worth nothing to somebody who copies it and the PIN itself never crosses the network |
| 2026-09-06 | A till fetches its people before its catalogue | A till with a catalogue and nobody able to sign in can ring nothing needing a permission, which is every refund and every drawer opening |
| 2026-09-06 | Settings fetches are recorded as asked, not inferred from what came back | A shop with nobody added yet answers with an empty list, and a driver reading that as "still does not know" asks again immediately, forever, for as long as the shop has one person working in it |
| 2026-09-06 | The whole set of people replaces the old set, never merges | Somebody removed from a shop has to disappear from the till, and a list that only grows leaves a departed cashier able to sign in for good |
| 2026-09-06 | Shop details are held on the device, fetched from the server | A receipt prints with the internet down, so they have to be there before they are wanted. Keeping them on each tablet instead means typing a BIN into every one and getting it wrong on the sixth |
| 2026-09-06 | A till learns what shop it is before it learns what it sells | A receipt with no name on it is one a customer cannot take back to anybody, and a catalogue arriving first would let the till sell anyway |
| 2026-09-06 | A shop with no name is refused rather than stored | It would print an empty line where the shop should be, which reads as a printer fault. Both stores refuse it, because a store that accepts what the other rejects is one tests pass against and production does not |
| 2026-09-06 | The credential lives in the terminal's standing state, not wherever a platform finds convenient | It belongs to the same thing as the ledger: wiping the till wipes the credential, and it survives a reload because the ledger does. A platform keeping its own copy is a second place for it to be stale, leaked, or lost |
| 2026-09-06 | A credential travels only attached to a request the core built | The platform never holds one, so it cannot send a stale one or forget to send any |
| 2026-09-06 | Cross-origin is a development setting, off by default, and never reflects the caller's origin | The shipped image serves the till and the admin app itself, so nothing is cross-origin. A server that echoes whichever origin asked has the appearance of a policy rather than one, which is worse because it stops anybody looking |
| 2026-09-06 | A sync step that cannot be decided is an error, not a decision to do nothing | Treating the two alike is how a till stops syncing with nobody being told, which is the failure the whole design is arranged against. It cost an hour of a screen that looked idle |
| 2026-09-06 | The platform posts bytes the core built and hands back bytes it received | A sync client written in Dart and again in JavaScript is two sets of retry rules, two cursor bugs and two ways to acknowledge a sale the server never stored. Requests are built and replies parsed in the same crate as the arithmetic being delivered |
| 2026-09-06 | A till pulls when it has never pulled, not only when told more is waiting | "More is waiting" is an answer only a previous pull can give. Without this a freshly enrolled till never asks, and it is the one till that has nothing and needs everything |
| 2026-09-06 | A pull counts as a pull even when it returns nothing | Otherwise a shop whose prices are settled asks again immediately, forever |
| 2026-09-06 | Sales sync before receipt numbers, and numbers before the catalogue | A catalogue can be pulled again tomorrow and a lease asked for again. A sale on a tablet that dies is gone, and running out of numbers degrades every later sale |
| 2026-09-06 | The sync driver has no way to express giving up | A shop cannot tell that a till stopped trying, and the sales are on a tablet nobody has backed up. It retries forever, slower and slower, to a ceiling |
| 2026-09-06 | Backoff doubles with no jitter, and the failure count saturates | Jitter exists to stop a thousand clients retrying in lockstep; these are a handful of tills per shop that did not start together, and randomness would mean this crate needs an entropy source it deliberately lacks. A counter that wrapped would take a week-long outage back to a one second retry |
| 2026-09-06 | Three targets are verified by building, not by assertion | The browser build runs and stores durably, the Android library links and exports the four expected symbols, and the server links the same core. What remains unrun is the Android library on an actual device |
| 2026-09-06 | A plain C ABI, not `flutter_rust_bridge` | A generator is a dependency that has to keep working across two toolchains it does not control, on the path a shop cannot do without. Four C functions have no generator, no version skew, and are callable from Dart, Kotlin or anything else |
| 2026-09-06 | One exported function carries every operation, as a JSON command | Two platforms bind to this differently, and a growing list of exports is a growing list of places for them to fall out of step. Adding an operation changes neither binding |
| 2026-09-06 | A null handle is answered with an error view, never a null pointer | A caller that must check for null before parsing has two failure paths to get right, and the one it forgets is the one that matters |
| 2026-09-06 | Browser dependencies are gated to wasm32 | Before that gate, an aarch64 build linked wasm-bindgen into the shared object a tablet would load |
| 2026-09-06 | Every OPFS read states its offset, never defaulting | A sync access handle carries an implicit position that a write advances, so a read after a write starts where the write ended and returns nothing. A blob written and read back in the same breath came back empty in Chrome, and a till would have booted on an empty ledger rather than refusing to boot |
| 2026-09-06 | OPFS reads and writes go through a buffer the browser owns | A view over the wasm heap is detached if the heap grows during the call, and whether bytes are copied back depends on binding details rather than on anything written here |
| 2026-09-06 | The storage self-test ships, rather than being deleted once it passed | A till that will not open is the worst thing that happens to a shop, and "this browser will not flush" versus "this ledger is corrupt" decides whether somebody restores a backup or buys a different tablet |
| 2026-09-06 | The FFI takes JavaScript numbers and refuses non-integral ones, rather than exposing i64 as BigInt | i64 crossing as BigInt makes every call site write `2000n` or fail at runtime with a message about BigInt rather than about the shop's data. Validating at the boundary keeps the integer invariant and says what is wrong in terms a caller can act on. Found by loading the module in Chrome, where the first scan threw |
| 2026-09-06 | The core is verified to compile for wasm32, not merely asserted to | "One core, three targets" was the load-bearing claim of the whole design and had never been built for two of them. It does compile, PBKDF2 and all, at 83.6 KB gzipped against a 400 KB budget |
| 2026-09-06 | The terminal is created before the code that enrols it exists | A redeemed code pointing at a terminal nobody created fails at the worst possible moment, with a shop standing there holding a new tablet |
| 2026-09-06 | The new device's id is minted by the device asking for the code | Identity is created where the work happens, as it is for sales and counts. Nothing waits on a server to be allowed to exist |
| 2026-09-06 | An enrolment code lasts at most an hour | Forty bits is ample for minutes and thin for a week, and a code that outlives the conversation it was read out in is a credential lying around |
| 2026-09-06 | Credentials carry a role, and the back office needs the owner one | Every enrolled device could reprice the catalogue, book deliveries and read the repair queue. A shop with six tills had six devices that were the whole shop, and any one left on a counter was as good as the keys |
| 2026-09-06 | Two roles, not a permission matrix | Two is what these shops have. Finer permission belongs to the person signing in at the till, which the core already models; this is about what the device may be used for, a different question needing a different answer |
| 2026-09-06 | The role check lives in the extractor, not in each handler | A forgotten check is how a till ends up able to reprice the shop, and a route added later is exactly where one gets forgotten |
| 2026-09-06 | An unrecognised role number reads as the weaker role | Guessing upward is how a rolling upgrade grants access nobody granted |
| 2026-09-06 | Existing credentials became owners in the migration | Before roles they could reach the back office, so that is what they were. Downgrading them silently removes access with nothing saying why, and would leave a tenant with no owner and no way to mint one |
| 2026-09-06 | Forwarded headers are read only when an operator says how many proxies sit in front | Behind Caddy or Cloudflare every request arrives from the proxy and shares one bucket of ten attempts a minute, so background noise locks every shop out of enrolling a tablet. Trusting the header blindly is worse: a caller invents an address and mints a fresh budget per request |
| 2026-09-06 | Renewal overlaps rather than revoking, and never extends a deadline | The renewal reply can be lost on a bad connection. Revoking the old credential the instant a new one is issued leaves the device holding nothing that authenticates and no way to ask for more. The overlap is a day; `least()` on the expiry stops a device renewing in a loop to hold one alive |
| 2026-09-06 | Terminal credentials expire after a year, and record when they were last used | A tablet sold on, lost, or handed back by a departing employee kept a working credential until somebody noticed, and these shops have nobody whose job that is. Existing tokens are not backfilled with an expiry: taking every live till offline at deploy time is worse than the risk |
| 2026-09-06 | A receipt number is claimed against a primary key, not checked with a read | The case a duplicate check exists for is a tablet restored from a backup, and a restored tablet pushes its whole backlog at once beside the device it was copied from. Two separate transactions both read "free" and both stored clean, at exactly the moment the check mattered |
| 2026-09-06 | The claim is its own table, not a unique index on `sale` | A unique index would refuse the second sale outright, and refusing is what the design rejects everywhere else: the goods left the shop. The sale always stores; only the claim can fail |
| 2026-09-06 | Stock movements are recomputed server-side from the ticket lines | The sent movements were believed, so a payload whose totals recompute perfectly could decrement any item it liked, or none, and sail through the tamper check |
| 2026-09-06 | An undecodable catalogue row is skipped and counted, not fatal | Failing the page made one bad row a permanent poison pill: a backend error, a 503, and every till in that shop stopped syncing with no way past it |
| 2026-09-06 | PBKDF2 for cashier PINs, not SHA-256, and not argon2 | A four to six digit PIN is a few hundred thousand candidates, so a stolen tablet cracks a fast hash in under a second. Argon2's memory hardness is the better property and its memory cost is the one thing a 2 GB tablet cannot spare. Rounds travel with each credential so the cost can be raised without locking anyone out |
| 2026-09-06 | A supervisor's authorisation is single use and expires in ninety seconds | Anything longer and the supervisor is effectively signed in at a till they walked away from, which is how every discount after lunch inherits their authority |
| 2026-09-06 | A sale is never refused for want of an open drawer | A till that will not sell because nobody pressed the right button in the morning is a till the shop works around, and a worked-around control protects nothing |
| 2026-09-06 | Standing terminal state lives in a blob slot, never in the critical log | Acknowledging every outstanding sale empties that log, which is the ordinary end of a trading day. Leased receipt numbers and parked baskets kept there went with it, so a shop that synced last night opened next morning, offline, with no numbers to print |
| 2026-09-06 | Lease recovery walks the blocks rather than advancing the active one | A till that crossed a block boundary offline came back with the spent block active and the block it had been selling from in reserve at its first number, and reprinted numbers already in customers' hands |
| 2026-09-06 | Which amount tax is charged on is a per-item choice, not a rule in the arithmetic | Two shops can both be right. Ordinarily a discount reduces the consideration and the tax with it; under a listed-price regime the tax is fixed to the price on the packet and the shop funds the discount from its own margin. The item says which, and the choice is frozen onto the line when it is rung |
| 2026-09-06 | The item schema went to version 2, with version 1 still readable | postcard is positional, so a field added to the item shape cannot be read out of old bytes. The old struct is kept solely to be decoded and converted, never written, which is what the schema numbers were put there for |
| 2026-09-06 | A discount reduces the taxable amount, so VAT is recomputed after apportionment | Leaving the pre-discount VAT charged the customer tax on money they did not pay and over-declared it to the revenue, and made a line discount and a ticket discount of the same size disagree |
| 2026-09-06 | Stock movements are summed per item before leaving the till | The cart opens a second line for the same item when the first is discounted, and the server keys a movement on the sale and the item, so per-line movements silently lost all but the first |
| 2026-09-06 | A failed rollback poisons the journal instead of continuing | The uncommitted frame stays in the log and the next successful commit flushes it, so a basket the cashier re-rang syncs twice. A till that stops is a phone call; one that bills twice is a dispute nobody notices |
| 2026-09-06 | Unreadable bytes are copied aside before recovery truncates them | Truncation is the last moment anybody could recover a committed, printed, unsynced sale sitting behind mid-log corruption |
| 2026-09-06 | Any failure to revalidate a synced sale quarantines it | Reading a decode error or an overflow as agreement meant bypassing the tamper check only required breaking the arithmetic rather than the total |
| 2026-09-06 | postcard on disk, wire types separate from domain types | postcard is positional: adding a field to `Item` turns every old snapshot into garbage. Disk needs backward compatibility, the sync wire needs forward compatibility too, and one struct serving both makes a wire change force a disk migration |
