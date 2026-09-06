-- Drawers that have been counted and closed.
--
-- The whole point of counting a drawer is that somebody who was not standing at
-- the till reconciles it. Until this table existed a cashier counted, the till
-- worked out the variance, and the owner had to take their word for both.
--
-- Immutable once written. A shift that has been counted and signed off is a
-- statement about a period that has ended; a correction is a cash movement in
-- the next one, not an edit to this.
create table if not exists closed_shift (
    tenant_id            uuid   not null references tenant (id) on delete cascade,
    id                   uuid   not null,
    terminal_id          uuid   not null,
    opened_at_ms         bigint not null,
    closed_at_ms         bigint not null,
    opening_float_minor  bigint not null,
    sales                integer not null,
    cash_sales_minor     bigint not null,
    non_cash_sales_minor bigint not null,
    cash_in_minor        bigint not null,
    cash_out_minor       bigint not null,
    -- What the drawer should have held, and what was in it.
    expected_cash_minor  bigint not null,
    counted_cash_minor   bigint not null,
    -- Counted less expected. Negative is short. Stored rather than recomputed,
    -- because it is what the person who counted was told at the time.
    variance_minor       bigint not null,
    received_at          timestamptz not null default now(),
    primary key (tenant_id, id)
);

create index if not exists closed_shift_by_day
    on closed_shift (tenant_id, closed_at_ms desc);

alter table closed_shift enable row level security;
alter table closed_shift force row level security;

create policy tenant_isolation on closed_shift
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on closed_shift to openpos_app;
    end if;
end
$$;
