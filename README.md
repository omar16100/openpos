# openpos

Offline-first point of sale for small retail shops, built for places where the internet is a thing
that comes and goes. The catalogue, the open basket, the counted drawer and the receipt numbers all
live on the device, so a till keeps selling with the line down and the shop reconciles when it comes
back.

**Status: pre-release.** The core, the server, the till screen and the back office all run and have
been driven end to end against real Postgres and in a real browser. It has never run in a shop. See
[What is not done](#what-is-not-done) before considering it for one.

## Licence

AGPL-3.0-only: the GNU Affero General Public License, version 3, and no later version. `LICENSE` is
the licence text exactly as the FSF publishes it at <https://www.gnu.org/licenses/agpl-3.0.txt>, and
every crate declares `AGPL-3.0-only` through the workspace `Cargo.toml`. The "or any later version"
wording near the end of `LICENSE` belongs to the FSF's sample notice for other programs and is not
this project's grant. Free to self-host forever; a managed cloud tier is the intended commercial
side, on the same open-core footing Postiz uses. None of that hosted tier is in this repository.

## How it is put together

Every rule that decides money lives in one Rust crate. The platforms are thin.

| Piece | What it is |
|---|---|
| `core/` | `openpos-core`. Integer money, pricing and VAT, the cart state machine, receipt layout, the on-device journal and snapshots, the sync driver, PIN auth and permissions. No unsafe, no unwrap, no unchecked arithmetic (see the workspace lints in `Cargo.toml`) |
| `bindings/` | `openpos-bindings`. The core as one WASM module, JSON in and JSON out, holding no rules of its own |
| `ffi/` | `openpos-ffi`. The same facade as four C functions, so an Android UI calls it with no code generator in the build |
| `server/` | `openpos-server`. Axum, postcard bodies, Postgres with row level security carrying the per-shop boundary. Also the export and import CLI |
| `apps/till-web/` | The till screen. Svelte 5 and Vite, the core in a web worker on OPFS |
| `apps/admin/` | The back office. Shop details, people, catalogue, stock, suppliers, accounts, reports |
| `apps/shared/` | The handful of rules both screens need, with `node --test` tests |

The same core answers a browser and a tablet, so the two cannot disagree about what a discount, a
tax base or a receipt number means.

## Run it

The shortest thing that works, an in-memory shop that does not survive a restart:

```sh
cargo run -p openpos-server
```

The whole thing the way a shop would run it, api, Postgres and the backup sidecar, till on
`http://localhost:8080/` and back office on `http://localhost:8080/admin/`:

```sh
docker compose up --build
```

`docs/running.md` is the guide: every setting, how to build and serve the two apps, how to run the
tests including the ones that silently skip without a database, and the example clients that reach
the failure paths a screen cannot get to.

## Tests

```sh
cargo test --workspace
node --test 'apps/shared/*.test.js'
```

CI (`.github/workflows/ci.yml`) runs both on every pull request and every push to `main`, the Rust
suite against a Postgres 16.9 service set up with `scripts/init-db.sql`, so none of it returns early.

The Postgres tests return early without `OPENPOS_TEST_ADMIN_DATABASE_URL` and
`OPENPOS_TEST_DATABASE_URL`, and one test in each of those files fails on purpose to say so, because
a skip reported as a pass is a failure nobody looks for.

Guards here are checked by breaking them one at a time and watching a test fail. The method, and
which guards are worth re-checking after a change, is written down in `docs/running.md`.

## What works

Selling with the line down and syncing after; leased receipt numbers with epoch fencing; VAT
inclusive and exclusive with a per-item tax base; discounts with ceilings and supervisor override;
refunds; parked baskets; split tenders including wallets, card and on account; the account book with
paged balances; shifts with X and Z reports and drawer variance; stock movements, deliveries,
suppliers, stock counts as ledger barriers and manual corrections; a repair queue for sales that
need a person; struck-out sales that every figure stops counting; export and import of one shop as a
file; device enrolment, credential renewal and revocation; and a printed receipt laid out once in
the core and rendered as ESC/POS bytes or as paper from a browser.

Measured on one developer machine, not on a shop's tablet or a shop's connection: 4.6 ms to make one
sale durable on files with a flush each, and 1.04 ms a sale to drain a day into Postgres. Both are
reproducible with the example clients in `docs/running.md`. The WASM module was 74.3 KB gzipped when
it was last measured in a browser, which `todo.md` records.

## What is not done

Stated here rather than left for somebody to find out in a shop. `todo.md` carries the full list
with the reasoning.

- The Android library builds and links for ARM64 but has never been run on a device or an emulator.
- No physical thermal printer has been near this. The ESC/POS byte stream is right by inspection and
  by the specification, and nothing on any screen sends it yet.
- Bengali cannot be printed on thermal paper. Those lines are marked and reported rather than sent
  as bytes that would print as mojibake. A screen renders them fine.
- The receipt is not a verified Mushak 6.3 tax invoice and makes no compliance claim. The NBR rules
  used here are vendor-blog sourced and have not been checked against a primary source.
- Exempt and zero-rated are not told apart, and a line carries one tax rate, so a supplementary duty
  stacked before VAT is not expressible.
- No billing.
- Backups stay on the machine that took them. The `backup` service in `docker-compose.yml` exports
  the shop named by `OPENPOS_SHOP` when it starts and once a day after that, reads each file back
  with `openpos-server verify`, and keeps the newest 14 in the `openpos-backups` volume. Nothing
  copies them anywhere else, and with `OPENPOS_SHOP` unset it takes no backup and says so in its log.
- TLS is off unless asked for. The Caddy terminator runs only under the `tls` compose profile
  (`docker compose --profile tls up -d`, with `OPENPOS_TRUSTED_PROXY_HOPS=1`). Without a public
  domain it signs with its own authority, which every tablet has to be told to trust. See
  `docs/running.md`.
- On macOS `flush` is weaker than on Linux, because the call that really waits needs unsafe and the
  workspace forbids it outside the C ABI. Android and Linux get a real barrier.

## Documentation

- `docs/index.md` the index and the conventions
- `docs/06092026_openpos_feature_spec.md` the v1 feature inventory, scope and architecture decisions
- `docs/c4model.md` containers, components, data flows and the decisions log, and the source of
  truth for architecture changes
- `docs/running.md` running it, the settings, the tests, and reaching the failure paths
- `todo.md` what has been done, each item with the defect or decision behind it, and what is open

## Contributing

Issues and pull requests are welcome. A contribution agreement is not set up yet, so anything
merged before one exists is taken under the AGPL the repository already carries.
