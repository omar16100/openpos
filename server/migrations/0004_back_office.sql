-- Back office: working the repair queue, and knowing which tills are alive.
--
-- Two facts the schema could not hold before this. First, whether a quarantined
-- sale has been dealt with: without it the queue only ever grows, every reload
-- shows the same rows, and the one shop with a broken till is indistinguishable
-- from a shop that has fixed everything. Second, when a terminal was last heard
-- from: `enrolled_at` says a device once existed, not that it still syncs, and
-- the difference is the whole of a support call.

-- Resolution is recorded beside the sale, never in place of it. The sale
-- happened, the payload is what a dispute is settled against, and the quarantine
-- reason stays so the record still says why a human was needed. Marking rather
-- than deleting is also what makes the queue auditable months later.
alter table sale add column if not exists resolved_at timestamptz;
alter table sale add column if not exists resolution  text;

-- Null for every terminal enrolled before this migration, and deliberately not
-- backfilled to now(). Claiming a device was heard from at deploy time would
-- report a dead till as healthy, which is the exact failure this column exists
-- to surface.
alter table terminal add column if not exists last_seen_at timestamptz;

-- The queue reads unresolved rows only, so the index has to say so too. The old
-- partial index covers quarantined rows whether or not they are resolved, which
-- would make the queue scan and discard everything the shop already worked
-- through, and that set only grows.
drop index if exists sale_quarantined;
create index if not exists sale_repair_queue
    on sale (tenant_id, received_at)
    where quarantine is not null and resolved_at is null;

-- Terminal health counts a terminal's sales. Without this the count is a scan of
-- every sale in the shop, once per terminal, on a page an owner reloads.
create index if not exists sale_by_terminal on sale (tenant_id, terminal_id);

-- No new tables, so no new policies: `sale` and `terminal` are already under
-- FORCE row-level security with both USING and WITH CHECK, and a new column on
-- an existing table inherits both that and the table-level grants made in 0001.
