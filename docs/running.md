# Running openpos

**Purpose.** Everything needed to start the server, the till and the back office, and to reach the
parts that only appear when something has gone wrong.
**Status.** Current, and true of the code at the date below rather than of any released version.
**Last updated.** 2026-09-07.

Until now these settings lived in code comments and in `todo.md`, which meant nobody could run this
without reading the source.

## The shortest thing that works

```sh
cargo run -p openpos-server
```

An in-memory shop with a catalogue, an owner, stock, and two enrolment codes printed to the log.
Nothing survives the process, which is what the log says in its first line. Good for looking at the
screens, useless for anything that has to outlive a restart.

Serve the two apps beside each other, because the back office expects to live under `/admin/`:

```sh
cd bindings && wasm-pack build --target web --release --out-dir ../target/pkg && cd ..
cp -r target/pkg apps/till-web/public/pkg
cp -r target/pkg apps/admin/public/pkg
(cd apps/till-web && npm install && npm run build)
(cd apps/admin    && npm install && npm run build)
cp -r apps/admin/dist apps/till-web/dist/admin
(cd apps/till-web/dist && python3 -m http.server 8100 --bind 127.0.0.1)
```

The apps look for the server on port 8099 of whatever host serves them, so the server needs to be
told the apps' origin is allowed:

```sh
OPENPOS_LISTEN=127.0.0.1:8099 OPENPOS_DEV_ALLOW_ORIGIN=http://127.0.0.1:8100 cargo run -p openpos-server
```

Read the till code onto `http://127.0.0.1:8100/` and the back office code onto
`http://127.0.0.1:8100/admin/`. Two codes for two terminals, and they are not interchangeable: each
device keeps its store in a directory named for its terminal, so two devices enrolling as one
terminal fight over the same files and fail with a complaint about access handles that says nothing
about the cause.

Sign in as **Demo Owner**, PIN **1234**. Demo data only; a real shop sets its own.

## A shop that survives a restart

```sh
docker compose up -d db

OPENPOS_DEMO=1 \
OPENPOS_LISTEN=127.0.0.1:8099 \
OPENPOS_DEV_ALLOW_ORIGIN=http://127.0.0.1:8100 \
OPENPOS_ADMIN_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5433/openpos \
OPENPOS_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
cargo run -p openpos-server
```

The seed is idempotent: a second start says the shop is already there and leaves it alone. Anything
that has to outlive a restart, which is most of what this product claims, can only be checked this
way.

## Settings

| Variable | Effect |
|---|---|
| `OPENPOS_LISTEN` | Address to bind. Defaults to `0.0.0.0:8080` |
| `OPENPOS_DATABASE_URL` | Connect to Postgres as the application role. Absent means an in-memory store and a demo shop |
| `OPENPOS_ADMIN_DATABASE_URL` | Run migrations with a role that may change the schema. Absent assumes the schema is already current |
| `OPENPOS_DEMO` | Seed a demo shop into whatever store is configured. Ignored when the shop is already there |
| `OPENPOS_DEV_ALLOW_ORIGIN` | Allow one cross-origin caller, for a browser app served from another port. A development setting, and the server says so on every start |
| `OPENPOS_TRUSTED_PROXY_HOPS` | How many proxies sit in front. Zero rate-limits by socket address; set it to 1 behind Caddy or Cloudflare, or every client shares one bucket |

`OPENPOS_DEMO` writes a catalogue nobody ordered and a person nobody hired, and the PIN is in the
source. It is for a demonstration or a test database, not a shop.

## Tests

```sh
cargo test --workspace
```

That runs, and **forty eight tests inside it skip silently while still reporting as passed**: the
forty two in `server/tests/postgres_repo.rs` and the six in `server/tests/export_import.rs`, all of
which want a database. To run them for real:

```sh
OPENPOS_TEST_ADMIN_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5433/openpos \
OPENPOS_TEST_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
cargo test --workspace
```

Confirm nothing skipped, because the count alone will not tell you:

```sh
cargo test --workspace -- --nocapture 2>&1 | grep -c skipping
```

The browser side has tests too, for the rules that are not in Rust:

```sh
node --test 'apps/shared/*.test.js'
```

No test runner is installed for them. They are assertions about pure functions,
and a dependency there is a dependency in the thing a shop runs.

Some behaviour differs between the two stores in ways only the real one shows: `sum()` over a
`bigint` column answers in `numeric`, and reading that as an `i64` is a panic rather than a wrong
number. That was found by a Postgres test and could not have been found by any other.

## Reaching the failure paths

The duplicate-receipt check, the totals check and the repair queue they feed only appear when
something has gone wrong, and a browser cannot get there: a screen cannot ring one sale twice under
one number, and it cannot tamper with a payload the core has just written.

```sh
cargo run -p openpos-server --example restored_till -- http://127.0.0.1:8099 <till-code>
```

A client, not a back door: an ordinary enrolment code and the ordinary push endpoint, behaving like
a device somebody restored from Friday's backup. It quarantines two sales and leaves them in the
back office queue.

A counted drawer crosses two devices, so it is checked the same way: a till counts one and the owner
reads it back, over the real endpoints against whatever store the server is using.

```sh
cargo run -p openpos-server --example counted_drawer -- http://127.0.0.1:8099 <till-code> <owner-code>
```

It prints what the till made of the count and what the back office sees, including who counted it.

Selling on account crosses the same two devices, and the second half of it happens weeks later:

```sh
cargo run -p openpos-server --example on_account -- http://127.0.0.1:8099 <till-code> <owner-code>
```

A till sells part cash and part on account, the owner reads what is owed, takes a payment, sends the
same payment twice on purpose, and reads back what the balance is made of.

A till the shop will not take sales from is the state nothing could fix before this week:

```sh
cargo run -p openpos-server --example carried_in -- http://127.0.0.1:8099 <till-code> <owner-code>
```

It rings two sales, is refused when it pushes, reads what it is holding off itself, and the owner
takes them in by hand. They land in the queue a person works, which is where a sale that arrived
without a credential behind it belongs.

Working the queue is the other half of that, and it is where the figures move:

```sh
cargo run -p openpos-server --example rung_twice -- http://127.0.0.1:8099 <till-code> <owner-code>
```

The same basket goes through twice, as it does on a tablet restored from an old backup. It prints
the takings, the tax, what is owed and what left the shelf, the owner says the second one never
happened, and it prints all four again. On the demo shop they halve: two sales for 197800 become
one for 98900, 25800 in tax becomes 12900, 4000 milli off the shelf becomes 2000. Both sales are
still in the database afterwards, which is the point.

Then it does what a shop does the next morning: reads the list of what was decided, finds that it
struck out the wrong one, changes the answer, and the four figures come back. Both answers stay in
`sale_resolution`, oldest first.

## Taking a backup

Everything one shop owns, as a file:

```sh
OPENPOS_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
cargo run -p openpos-server -- export <shop-id> > shop.jsonl
```

The shop id is the one its own logs and its own bundle use. Logs go to stderr and
the bundle to stdout, so a redirect gives a file that reads back.

One line per record, ending in a trailer stating what should have been in it: a
file cut short by a full disk fails to read rather than importing two thirds of a
shop and reporting success. Everything append-only is cut at the database's clock
when the export starts, so a shop trading through its own backup produces a file
describing one moment rather than a mixture: a sale that lands mid-export is left
out whole, its stock movements and its account entries with it.

Credentials are deliberately not in it.

## Things worth knowing before you are surprised by them

- Finishing a sale opens the browser's print dialog, which blocks the tab until it is dismissed.
  Correct for a shop, awkward when driving the screen from a script.
- A catalogue change reaches a device within about thirty seconds; shop details and people take up
  to ten minutes. Both screens say so where it matters.
- A till and the back office served from one origin share an OPFS root. They stay apart because each
  keeps its store in a directory named for its terminal, which is why the two demo codes exist.
