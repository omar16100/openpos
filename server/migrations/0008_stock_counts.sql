-- Counting the shelf, in a way a late sale cannot quietly undo.
--
-- A stock count is not a movement. It is an assertion that at one moment the
-- shelf held exactly this much, and everything the ledger believed before that
-- moment is superseded. That makes it a barrier: on-hand is the counted figure
-- plus whatever moved after it, and never a running total stretching back to
-- the day the shop opened.
--
-- The hazard this table exists to handle is a till that was offline. A sale rung
-- at nine in the morning can arrive at six in the evening, after somebody
-- counted the shelf at noon. Ordering the ledger by the device's clock lets that
-- sale insert itself before the count and rewrite it. Ordering by arrival
-- applies it after the count, decrementing stock the counter already saw was
-- gone. Both are wrong, and neither is detectable afterwards.
--
-- So both times are kept. `counted_at_ms` is when the person counted, by the
-- device's clock, and decides which sales the count should already reflect.
-- `recorded_at` is when the server learned of it, and decides which sales
-- arrived too late to have been included. A sale rung before the count but
-- arriving after it is in neither camp: nobody can say whether the counter saw
-- those goods. It is excluded from on-hand and raised for a person, because a
-- guess that goes unrecorded is worse than a discrepancy that gets looked at.
create table if not exists stock_count (
    tenant_id     uuid   not null references tenant (id) on delete cascade,
    id            uuid   not null,
    item_id       uuid   not null,
    counted_milli bigint not null,
    -- Device clock at the moment of counting. Never trusted for ordering
    -- between terminals, only for deciding what this count should have seen.
    counted_at_ms bigint not null,
    -- Which terminal or back-office session recorded it, so a count that looks
    -- wrong can be traced to whoever took it.
    counted_by    uuid   not null,
    -- Server arrival: the barrier itself.
    recorded_at   timestamptz not null default now(),
    -- Free text from the person counting. A count with no explanation is
    -- indistinguishable from a mistake when the variance is read a week later.
    note          text,
    primary key (tenant_id, id)
);

-- On-hand asks for the newest count of one item, constantly. Descending on the
-- device clock, because that is the count that supersedes the others.
create index if not exists stock_count_latest
    on stock_count (tenant_id, item_id, counted_at_ms desc);

alter table stock_count enable row level security;
alter table stock_count force row level security;

create policy tenant_isolation on stock_count
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

-- Deriving on-hand means selecting a sale's movements by when it was rung and
-- when it landed, which without this is a scan of every sale in the shop.
create index if not exists sale_rung_and_received
    on sale (tenant_id, rung_at_ms, received_at);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete on stock_count to openpos_app;
    end if;
end
$$;
