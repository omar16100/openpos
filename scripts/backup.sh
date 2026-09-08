#!/bin/sh
# Take a shop's nightly backup, check it reads, and keep the last few.
#
# A shop that self-hosts has one copy of everything it has ever sold, on one
# machine, in one Postgres volume. The export has existed since the week it was
# needed; what has not existed is anything that runs it while nobody is
# watching, which is the only kind of backup that gets taken.
#
#   sh scripts/backup.sh <shop-id> [where]
#
# Three things in one order, and the order is the point:
#
#   1. Write to a part-file, so a run that dies half way leaves something with
#      a name nobody will mistake for a backup.
#   2. Read it back with `openpos-server verify`, which parses it exactly as a
#      restore would. A truncated bundle looks like a whole one until the
#      morning somebody needs it: same name, same place, plausible size.
#   3. Only then give it its real name, and only then delete the oldest.
#
# Nothing here talks to the database except through the server's own export, so
# a backup is the shop as the application understands it rather than as the
# schema happens to store it today.
set -eu

shop="${1:?which shop? give the id it is known by}"
where="${2:-/backups}"
keep="${OPENPOS_BACKUPS_KEPT:-14}"
server="${OPENPOS_SERVER_BIN:-openpos-server}"

mkdir -p "$where"
stamp="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
part="$where/$shop-$stamp.jsonl.part"
whole="$where/$shop-$stamp.jsonl"

echo "taking a backup of $shop"
"$server" export "$shop" > "$part"

echo "reading it back before trusting it"
"$server" verify < "$part"

mv "$part" "$whole"
echo "kept $whole"

# Oldest first, and only whole ones: a part-file left by a run that died is not
# a backup and must not be counted as one when deciding what to delete.
count=$(ls -1 "$where"/"$shop"-*.jsonl 2>/dev/null | wc -l | tr -d ' ')
if [ "$count" -gt "$keep" ]; then
    over=$((count - keep))
    ls -1 "$where"/"$shop"-*.jsonl | sort | head -n "$over" | while read -r old; do
        echo "dropping $old"
        rm -f "$old"
    done
fi
