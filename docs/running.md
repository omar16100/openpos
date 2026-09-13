# Running openpos

**Purpose.** Everything needed to start the server, the till and the back office, and to reach the
parts that only appear when something has gone wrong.
**Status.** Current, and true of the code at the date below rather than of any released version.
**Last updated.** 2026-09-13.

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
sh scripts/stage-apps.sh /tmp/openpos-apps
(cd /tmp/openpos-apps && python3 -m http.server 8100 --bind 127.0.0.1)
```

The script is the four steps below in the one order that works. Doing three of them is worse than
doing none: an app rebuilt against a stale core boots, looks right, and fails on whatever command
the new core added, which has cost two browser sessions spent looking for a bug in a screen that was
fine.

```sh
cd bindings && wasm-pack build --target web --release --out-dir ../target/pkg && cd ..
cp -r target/pkg apps/till-web/public/pkg
cp -r target/pkg apps/admin/public/pkg
(cd apps/till-web && npm install && npm run build)
(cd apps/admin    && npm install && npm run build)
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

The two URLs are not interchangeable. The one the server serves on must be the unprivileged role:
row level security is the whole of the shop boundary here, and a role that bypasses it, which a
superuser does by definition, has no boundary at all. The binary refuses to start on one, and refuses
to export on one, because the mistake is a single word in a connection string and looks exactly like
a server that works.

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

Every test that is not beside the code it tests lives in its crate's `tests/suite/` and is a module
of one `tests/main.rs`. One file per concern, and the file names are the documentation, as before:
what changed is that they are modules of one binary per crate rather than a binary each. A new test
file goes in `suite/` and gets a line in `main.rs`, and `cargo test -p openpos-core what_it_is_called`
still runs one of them by name.

The reason is measured. Linking is what a run of this suite costs: a change to the core used to
relink thirty one binaries of about 21 MB each, and a full run took 3,619 seconds of which five
were spent running tests. The same run is 1,071 seconds now.

That runs, and **forty eight tests inside it skip silently while still reporting as passed**: the
forty two in `server/tests/postgres_repo.rs` and the six in `server/tests/export_import.rs`, all of
which want a database. To run them for real:

```sh
OPENPOS_TEST_ADMIN_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5433/openpos_test \
OPENPOS_TEST_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos_test \
cargo test --workspace
```

`openpos_test`, not `openpos`, and the reason is not speed. A run of the suite leaves thousands of
shops behind: every test that needs one makes one and nothing tidies up, which is right for a test.
Pointed at the database a demo shop lives in, it buries that shop, and an hour went into reading the
wrong figure off that table: sixteen thousand tenants and twenty thousand sales, of which the two
hundred and fifty somebody wanted were one shop's.

It is not about speed, which was the first guess and was wrong. Measured both ways, the
postgres-backed tests take the same half second against a database holding sixteen thousand shops as
against an empty one. What makes a full run long is compiling and starting thirty-one test binaries,
not querying.

Without them, one test in each of those two files fails on purpose and says so. Everything else in
them returns early, which the harness reports as a pass, so the failure is the only thing standing
between a green suite and a suite that tested nothing about the database.

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

On the screens, that device can also save what it is holding to a file and copy it to the clipboard,
and both ends show the same mark: eight hex digits of the bundle's own CRC-32, so a paste that got
cut short is caught by two people reading four characters to each other. The back office opens the
file directly, and accepts a paste a messaging app has wrapped.

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

Who allowed what crosses the same two devices, and is the record that answers the question asked
after a variance:

```sh
cargo run -p openpos-server --example who_allowed_it -- http://127.0.0.1:8099 <till-code> <owner-code>
```

A cashier who may not discount is allowed one by a supervisor, takes cash out on her own permission,
then somebody types her PIN wrongly twice. The till sends all four, sends them again as a till does
when a reply goes missing, and the owner reads them back with the names attached. The shop holds
four records, not eight.

What a shop's stock rule does at the counter, which is two devices and a shelf:

```sh
cargo run -p openpos-server --example past_the_shelf -- http://127.0.0.1:8099 <till-code> <owner-code>
```

The owner sets the rule to refuse, the till fetches it with the shop's own details, asks what the
shelves hold, and is then stopped ringing one more than the shop has. A supervisor allows it for
that basket, and the screen still says the shelf disagrees. Against the demo shop, whose opening
delivery is forty of everything:

```text
the till asked what the shelves hold and took 7 figures
the shelf holds 40 Rice Miniket 5kg
  rang 40: taken
  rang one more: refused, "the shop has 40 Rice Miniket 5kg and this basket wants 41"
  the supervisor allowed it: taken
```

It puts the shop back the way it found it, so running it twice is the same as running it once.

What a day offline costs to send, which nothing had measured:

```sh
cargo run --release -p openpos-server --example long_day -- http://127.0.0.1:8099 <till-code> 300
```

It rings the sales with nothing sent, then drains them in batches of 25 the way the driver does, and
prints what each batch cost. On this machine against Postgres, in release: 300 sales in 12 batches
in 0.31 s, 1.04 ms a sale; 1500 in 60 batches in 1.58 s, 1.06 ms a sale. Flat, so the size of the
day is not the problem. What it does not measure is the line a shop is actually on, which adds a
round trip per batch, or what a device costs to write each sale to its own storage: that one lives
in the browser and is measured there.

## The whole thing, as a shop would run it

```sh
docker compose up --build
```

The api and Postgres, with the till on http://localhost:8080/ and the back office on
http://localhost:8080/admin/. The image carries both apps and serves them itself, so there is no web
server to configure and nothing is cross-origin. `OPENPOS_DEMO` is on in that file: take it out for
a real shop, or it seeds a catalogue nobody ordered and a person nobody hired.

`docker compose down -v` takes the volume with it, which is the same database the Postgres tests
run against. `down` without the flag stops the containers and keeps it.

The two database URLs in it are two roles on purpose. Migrations run as the owner, and everything a
till or a back office asks for goes through the unprivileged one, which is the only role the
isolation policies apply to.

## What the log says

The server writes a line for every act that moves money or trust, so a shop's own log answers the
first questions anybody asks it. From a real run of `--example rung_twice`:

```text
INFO a device enrolled and was given a credential tenant=1 terminal=2
INFO a block of receipt numbers was issued tenant=1 terminal=2 epoch=1 first=1 last=50
INFO sales taken from a till tenant=1 terminal=2 carried=25 accepted=25 quarantined=0
INFO a counted drawer reached the shop tenant=1 terminal=2 drawer=500 counted_by=Rahima \
     expected_minor=123900 counted_minor=119900 variance_minor=-4000
WARN a carried-in sale is waiting for somebody to decide tenant=1 sale=901 reason=CarriedIn
WARN a credential this shop does not hold was presented
```

One line per push batch rather than per sale, because a till syncs all day. A credential is never in
a line, only that one was refused: a log is read by more people than a database. Every 503 the server
returns now names the line it came from, which is the difference between "the till says it cannot
reach the shop" and knowing which query gave up.

`RUST_LOG` sets the level, as usual: `RUST_LOG=openpos_server=debug` for everything, or
`RUST_LOG=warn` for only the things somebody has to act on.

## Checking that a guard is real

A test that passes when you break the code is not a test. The only way to know is to break it:

```sh
# take the condition out, run what should notice, put it back
cp server/src/pg.rs /tmp/pg.bak
# ... remove one predicate ...
OPENPOS_TEST_ADMIN_DATABASE_URL=... OPENPOS_TEST_DATABASE_URL=... cargo test -p openpos-server
cp /tmp/pg.bak server/src/pg.rs
```

Done to every guard added in the week this was written. Fourteen were caught by something; one was
not, and now is. The ones worth re-checking after any change near them: the struck-out sale filters,
the two clock impossibilities, the barcode refusal, PIN verification, the receipt-number cursor, the
binding of a credential to the shop in the body, the frame checksum, and the rollback that gives a
receipt number back when a commit fails.

Two whole-suite variants are worth running as well:

- With every explicit `tenant_id = $1` removed from `server/src/pg.rs`, every answer should be
  identical: row level security carries the boundary, and the predicates are belt and braces.
- With `OPENPOS_TEST_DATABASE_URL` pointed at the superuser, tests should fail rather than pass. A
  suite that cannot prove anything about isolation should say so, and one of them does.

What a sale costs to make durable, which is the figure that decides whether a queue moves:

```sh
cargo run --release -p openpos-bindings --example flush_cost -- 200
```

On this machine, against the file-backed store with a flush on every sale: 4.6 ms a sale, and 0.002
ms for the arithmetic on its own. Flat from fifty sales to five hundred. A cheap tablet's flash is
slower than any desk, so what transfers is the shape rather than the number: one flush per sale, no
growth with the length of the day.

## Putting it behind TLS

What crosses a shop's wifi between a till and the server is a bearer credential and the day's sales.
That wifi has one password, and the delivery man knows it.

```sh
OPENPOS_TRUSTED_PROXY_HOPS=1 OPENPOS_HOST=shop.example.com \
docker compose --profile tls up -d
```

Two ways to get a certificate, and the difference matters more than the configuration:

- A name that resolves to the machine, with 80 and 443 reachable, and `OPENPOS_TLS` set to anything
  other than `internal`. Caddy fetches a real certificate and renews it, and every tablet trusts it
  with no work at all.
- Anything else, which is the default. Caddy makes its own authority and signs for the name. No
  tablet trusts that until somebody installs the authority on each one, which is a real afternoon and
  the honest price of a shop with no domain.

`OPENPOS_TRUSTED_PROXY_HOPS=1` goes with it and is not optional. The server rate limits by the
caller's address; behind a proxy every request arrives from the proxy, so without it the whole shop
shares one bucket and one guessed enrolment code locks out every tablet in the building.

`OPENPOS_HTTP_PORT` and `OPENPOS_HTTPS_PORT` move the published ports for a bench where something
already holds 443. A certificate from a public authority needs the real ones.

## The nightly backup

A shop that self-hosts has one copy of everything it has ever sold, on one machine, in one Postgres
volume. The export has existed since the week it was needed; what was missing was anything that runs
it while nobody is watching, which is the only kind of backup that gets taken.

```sh
OPENPOS_SHOP=<shop-id> docker compose up -d backup
```

The sidecar runs the same image as the server, once at start-up and then daily. Each run writes to a
part-file, reads it back with `openpos-server verify` exactly as a restore would, and only then gives
it its real name and drops the oldest. A truncated bundle looks like a whole one until the morning
somebody needs it: same name, same place, plausible size. It keeps a fortnight by default,
`OPENPOS_BACKUPS_KEPT` says otherwise, and the files land in the `openpos-backups` volume.

By hand, or on a machine the files were copied to:

```sh
OPENPOS_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
sh scripts/backup.sh <shop-id> /some/where

openpos-server verify < shop.jsonl
```

`verify` needs no database. That is the point: a backup should be checkable where it was copied to
rather than only where it came from. It exits non-zero and says which line stopped it.

It checks two different things, and a file can pass the first and fail the second. Whole is every
line parsing and every id lining up. Sound is every sale stating the total its own bytes carry: what
a shop declared is recomputed from the payload on the way back in rather than read out of the file,
so a total edited in a text editor is a figure a restore would silently correct. `verify` refuses
such a file, because the nightly job above is built on it: the sidecar writes a part-file, reads it
back with `verify`, and only then gives it its real name and drops the oldest. A bundle that
disagrees with itself passing that gate is a good backup rotated away for a bad one.

`import` does the opposite and says so: it takes the figure from the bytes, restores the shop, and
tells you how many disagreed. A bundle with one figure wrong is still a shop's whole history, and
losing all of it to save one line is the worse trade. The gate refuses; the rescue carries on.

`openpos-server help` lists all four one-shot commands and what each takes. Anything it cannot read
prints the same list beside the complaint: `code --tenant <id>`, which is the shape every other tool
in the world takes, used to answer "--tenant is not a shop id" and say nothing about what would have
worked.

There is no automatic restore. Putting a shop back is `import`, below, and it is somebody's
deliberate act with the till in front of them.

A backup nobody has restored is not a backup. Last night's real file was put into a database that
had never held the shop, and what came back matched the file: 250 sales, 46 catalogue rows, 267
stock movements, 9 account entries, 4 counted drawers, 4 people, 3 customers, 2 suppliers, and the
takings to the poisha. Worth repeating on your own file occasionally, into a scratch database, which
costs one `create database` and proves the thing the nightly log can only assert.

## Getting back in when the back office device is gone

Every enrolment code comes from the back office, and the only owner's code a shop was ever given was
printed the first time the server started. A shop that loses that tablet a year later has a database
full of its own takings and no way to look at them.

```sh
OPENPOS_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
cargo run -p openpos-server -- code <shop-id>
```

The code goes to stdout and everything else to the log beside it, so it can be copied straight off
the screen. It lasts an hour, works once, and enrols the device as a new terminal, which is what a
replacement tablet is. Add `--till` for a till's code instead.

A subcommand rather than a route: it is an operator's act on the machine the database is on, and
whoever can run it can already read the database. From inside the back office, the list of devices
offers the same thing per device, and says which of them is the back office so the code it offers is
the right one.

## Taking a backup

Everything one shop owns, as a file: the tenant row and what it prints at the top of a receipt, the
terminals, the people who may stand at them, the people it buys from and what it owes them, the
catalogue history, the sales with their payloads, the stock movements with the counts and
corrections behind them, the account book, the counted drawers, and what the tills allowed. No
credential and no PIN.

```sh
OPENPOS_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
cargo run -p openpos-server -- export <shop-id> > shop.jsonl
```

The shop id is the one its own logs and its own bundle use. Logs go to stderr and
the bundle to stdout, so a redirect gives a file that reads back.

Putting one back:

```sh
OPENPOS_ADMIN_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5433/openpos \
OPENPOS_DATABASE_URL=postgres://openpos_app:openpos_app@127.0.0.1:5433/openpos \
cargo run -p openpos-server -- import < shop.jsonl
```

The admin URL is there because a restore is usually the first thing a machine is asked to do. A
rented box after the old one died, a replacement server, somebody proving the backup works: none of
them has the tables yet, and a restore given the admin URL makes them before it writes, the way
serving always has. Leave it out on a machine that is already serving and the restore works
unchanged; leave it out on a fresh one and the refusal says which variable to add and that nothing
was written.

The shop keeps the id it had, because the tills still hold sales carrying it. Add `--as <shop-id>`
to put a copy under a different one, which is what a duplicate for testing wants. Running it twice
changes nothing: every write is keyed on an identifier a till minted.

What the file deliberately does not carry is anybody's PIN. A four-digit PIN behind any number of
rounds is a few thousand guesses to whoever holds the file, so the people come back with their
permissions and their ids and a PIN nobody can type, and the import says so:

```text
WARN no PIN travels in a bundle: set one for each of these before anybody can sign in people=1
```

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
