# Licence text, README accuracy and CI

**Purpose.** Make `LICENSE` the licence text exactly as the FSF publishes it, bring the README in
line with what `docker-compose.yml` actually runs, take a personal path out of the docs, and add CI.
**Status.** Done.
**Last updated.** 2026-09-27.

## Context

- `LICENSE` differed from <https://www.gnu.org/licenses/agpl-3.0.txt> in four lines, all in "How to
  Apply These Terms to Your New Programs", the appendix after END OF TERMS AND CONDITIONS. The
  sample notice's two placeholder lines had been filled in with another project's name and
  copyright holder (Postiz, Nevo David), and the two lines after them were wrapped differently. The
  file had been taken from a copy on the development machine (commit `3e767e5`), and that copy was
  evidently Postiz's.
- Provenance was checked before changing it. `git log --all -S Postiz` finds three commits: the
  feature spec and the README, which name Postiz only as the open-core business model this project
  follows, and `3e767e5`, which added `LICENSE`. No source file in any commit mentions Postiz, and
  the code is Rust and Svelte written here. There is no upstream notice to preserve.
- The README said the file was the licence "verbatim", which was not true until now.
- The grant is `AGPL-3.0-only`: `license = "AGPL-3.0-only"` in the workspace `Cargo.toml`, inherited
  by all four crates, and the notices in `core/src/lib.rs` and `server/src/main.rs` say "version 3"
  with no "or later". The appendix's "either version 3 of the License, or (at your option) any
  later version" is FSF template text for other programs to copy, not a grant by this one. The
  README now says so. The two private `package.json` files had no licence field and now carry the
  same identifier.
- README "What is not done" said there was no TLS terminator and no backup sidecar. Both exist in
  `docker-compose.yml`: Caddy behind the `tls` profile, and a `backup` service that exports the shop
  named by `OPENPOS_SHOP` at start-up and daily after, verifies each file and keeps 14.
- `docs/index.md` and the feature spec pointed at absolute paths on the author's machine for work
  that is not published.
- There was no CI.

## Phases

1. Replace `LICENSE` with the downloaded FSF text, byte for byte (`cmp` clean).
2. README: licence section states the `AGPL-3.0-only` grant and explains the appendix; "What is not
   done" rewritten from `docker-compose.yml`; the compose line in "Run it" names the sidecar; the
   tests section says what CI runs. The timing figures are unchanged.
3. `docs/index.md` and `docs/06092026_openpos_feature_spec.md`: absolute paths replaced with a
   plain statement that those documents are not published.
4. `.github/workflows/ci.yml`: a Rust job runs `cargo test --workspace --locked` on ubuntu with
   `postgres:16.9` as a service, set up by `scripts/init-db.sql`, so the Postgres-backed tests run
   instead of returning early. A node job runs `node --test 'apps/shared/*.test.js'` on Dhaka
   time, which needs no install and takes seconds. Nothing in the workspace is excluded.
5. `docs/running.md`, `docs/c4model.md` (no longer "there is no CI", plus a decisions log row) and
   `todo.md` updated.

## Status log

- 2026-09-27: provenance checked, no Postiz code. `LICENSE` identical to the FSF file.
- 2026-09-27: locally, against a throwaway Postgres 17 cluster with the same init script,
  `cargo test --workspace --locked` passed 926 tests with 1 ignored (a measurement, not an
  assertion), and the one `skipping` line in `--nocapture` output was a test name rather than a
  skip. Without the database the two deliberate "needs a database" tests fail, as documented.
  `node --test 'apps/shared/*.test.js'` passed 271 under three time zones.
- 2026-09-27: external review found no blocker or major issue. One minor, applied: `days.test.js`
  returns early unless the clock is on Dhaka time, so the node job sets `TZ: Asia/Dhaka` rather than
  letting a UTC runner pass that test without checking anything. 271 passed with it set.

## Deviations

- The task said to replace the Postiz line with an openpos line. The canonical FSF placeholders
  were restored instead, because `LICENSE` is meant to be the licence text itself, and the
  project's own notice already lives in the two entry points' headers.
- CI does not build the WASM module, the Android library or the two apps' bundles. Those need
  `wasm-pack`, the NDK and npm installs, and stay the manual steps in `docs/running.md`.
