#!/bin/sh
# Build the two apps with a fresh core, and put them where a static server can
# serve them.
#
# Four steps in one order, because doing three of them is worse than doing none:
# an app rebuilt against a stale wasm boots, looks right, and fails on the one
# command the new core added. That has cost this project a browser session twice
# now, both times spent looking for a bug in a screen that was fine.
#
#   sh scripts/stage-apps.sh [where]
#
# The till lands at the root of `where` and the back office under /admin/, which
# is the layout OPENPOS_APPS expects and the one the shipped image uses.
set -eu

where="${1:-/tmp/openpos-apps}"
here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

echo "building the core for the browser"
(cd bindings && wasm-pack build --target web --release --out-dir ../target/pkg >/dev/null)

echo "giving each app its own copy of it"
rm -rf apps/till-web/public/pkg apps/admin/public/pkg
cp -r target/pkg apps/till-web/public/pkg
cp -r target/pkg apps/admin/public/pkg

echo "building the till and the back office"
(cd apps/till-web && npm run build >/dev/null)
(cd apps/admin && npm run build >/dev/null)

echo "staging them at $where"
rm -rf "$where"
mkdir -p "$where/admin"
cp -r apps/till-web/dist/. "$where/"
cp -r apps/admin/dist/. "$where/admin/"

echo "done: serve $where and the till is at / with the back office at /admin/"
