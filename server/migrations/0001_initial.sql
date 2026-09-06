-- openpos initial schema.
--
-- Identity is uuid throughout. A ULID is 128 bits and maps onto a uuid exactly,
-- so an id stays 16 bytes from the terminal that minted it to the row that
-- stores it, with no string conversion anywhere on the path.
--
-- Money is bigint in minor units and quantities are bigint in milli-units, the
-- same integers the till uses. No numeric, no float: an auditor re-adds these by
-- hand and every value must be exact.

create table if not exists tenant (
    id            uuid primary key,
    name          text        not null,
    -- Per-tenant replication cursor. Deliberately not a global sequence: a
    -- shared one would leak how busy other shops are, and would make a till's
    -- cursor jump for reasons that have nothing to do with its own catalogue.
    catalogue_seq bigint      not null default 0,
    created_at    timestamptz not null default now()
);

create table if not exists terminal (
    tenant_id    uuid   not null references tenant (id) on delete cascade,
    id           uuid   not null,
    label        text   not null default '',
    -- Bumped when the back office believes this device was replaced or restored
    -- from a backup, so numbers issued before and after stay distinguishable.
    epoch        bigint not null default 1,
    -- Next receipt number this terminal has not yet been granted.
    next_receipt bigint not null default 1,
    enrolled_at  timestamptz not null default now(),
    primary key (tenant_id, id)
);

-- Catalogue changes, in the order a till replays them.
create table if not exists catalogue_change (
    tenant_id uuid     not null references tenant (id) on delete cascade,
    seq       bigint   not null,
    -- 1 upsert, 2 delete. A smallint rather than an enum so adding a kind later
    -- does not require an exclusive lock on the type.
    kind      smallint not null,
    item_id   uuid     not null,
    -- postcard-encoded ItemWire for an upsert, null for a delete.
    payload   bytea,
    made_at   timestamptz not null default now(),
    primary key (tenant_id, seq)
);

-- Sales as received from tills.
create table if not exists sale (
    tenant_id     uuid   not null references tenant (id) on delete cascade,
    id            uuid   not null,
    terminal_id   uuid   not null,
    receipt_no    text,
    receipt_epoch bigint,
    rung_at_ms    bigint not null,
    total_minor   bigint not null,
    -- The bytes exactly as the terminal committed them, kept verbatim so a
    -- dispute is settled against what the till wrote rather than a re-encoding.
    payload       bytea  not null,
    -- Null when the sale needs nobody. Set means it is in the repair queue; the
    -- sale is stored either way, because refusing it would leave its only copy
    -- on a tablet.
    quarantine    text,
    received_at   timestamptz not null default now(),
    primary key (tenant_id, id)
);

-- Deliberately not unique. A duplicate receipt number is a finding to be
-- surfaced, not a row to be rejected: the sale already happened.
create index if not exists sale_receipt_lookup
    on sale (tenant_id, receipt_no, receipt_epoch)
    where receipt_no is not null;

create index if not exists sale_quarantined
    on sale (tenant_id, received_at)
    where quarantine is not null;

create table if not exists stock_movement (
    tenant_id uuid   not null references tenant (id) on delete cascade,
    sale_id   uuid   not null,
    item_id   uuid   not null,
    qty_milli bigint not null,
    primary key (tenant_id, sale_id, item_id)
);

-- Row-level security.
--
-- FORCE is the important word. Without it a table's owner bypasses its own
-- policies, which is how most deployments end up with RLS that silently does
-- nothing: it is enabled, it is never exercised, and nobody notices until a
-- query returns another shop's sales. FORCE subjects the owner too. A superuser
-- still bypasses, which is why the application must never connect as one.
alter table tenant           enable row level security;
alter table terminal         enable row level security;
alter table catalogue_change enable row level security;
alter table sale             enable row level security;
alter table stock_movement   enable row level security;

alter table tenant           force row level security;
alter table terminal         force row level security;
alter table catalogue_change force row level security;
alter table sale             force row level security;
alter table stock_movement   force row level security;

-- Every policy reads the same setting, which the application sets with
-- set_config(..., is_local => true) inside each transaction. An unset value
-- makes the cast yield null, the comparison fail, and the query return nothing,
-- so forgetting to scope a transaction is loud rather than silently
-- cross-tenant.
--
-- Both clauses are required, and the second is the one that is easy to forget:
-- USING governs which rows can be read, updated or deleted, while WITH CHECK
-- governs which rows may be written. A policy with USING alone silently refuses
-- every insert, because there is no rule permitting one.
create policy tenant_isolation on tenant
    using (id = current_setting('openpos.tenant_id', true)::uuid)
    with check (id = current_setting('openpos.tenant_id', true)::uuid);
create policy tenant_isolation on terminal
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);
create policy tenant_isolation on catalogue_change
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);
create policy tenant_isolation on sale
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);
create policy tenant_isolation on stock_movement
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);


-- Grant the application role what it needs, if it exists. Written as a
-- conditional so the same migration runs against a database that has no
-- separate role yet, such as a small self-hosted install.
do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant usage on schema public to openpos_app;
        grant select, insert, update, delete on all tables in schema public to openpos_app;
        grant select, update on all sequences in schema public to openpos_app;
    end if;
end
$$;
