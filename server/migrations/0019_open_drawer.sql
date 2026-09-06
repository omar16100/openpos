-- The drawer a till has open right now.
--
-- A counted drawer is sent when it closes, which is the only moment the shop
-- ever heard about one. A till left open overnight and wiped in the morning
-- took its whole takings summary with it, and an owner asking which tills still
-- had a drawer open had nowhere to look.
--
-- One row per terminal, replaced as the till reports. This is the last thing a
-- till said about a drawer that had not closed yet, not a record of anything
-- that happened: the closed drawer is the record, and it lives in
-- `closed_shift`. Deleted when that arrives, so an open list is a list of what
-- is actually open.
create table if not exists open_drawer (
    tenant_id            uuid   not null references tenant (id) on delete cascade,
    terminal_id          uuid   not null,
    shift_id             uuid   not null,
    opened_at_ms         bigint not null,
    -- When the till last said this. A figure from four hours ago is a different
    -- thing from one from four minutes ago, and only the shop can tell which
    -- matters.
    reported_at_ms       bigint not null,
    opening_float_minor  bigint not null,
    sales                integer not null,
    cash_sales_minor     bigint not null,
    non_cash_sales_minor bigint not null,
    cash_in_minor        bigint not null,
    cash_out_minor       bigint not null,
    expected_cash_minor  bigint not null,
    primary key (tenant_id, terminal_id)
);

alter table open_drawer enable row level security;
alter table open_drawer force row level security;

create policy tenant_isolation on open_drawer
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete on open_drawer to openpos_app;
    end if;
end
$$;
