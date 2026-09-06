-- Stock that left without being sold, and stock a count got wrong.
--
-- Breakage, spoilage, theft, a sample handed to a customer, a mistyped figure.
-- Movement kind 3 was reserved for this when the ledger stopped assuming every
-- movement was a sale; this is what writes one.
--
-- A separate kind from both a sale and a delivery, because the question a
-- shopkeeper asks at the end of a bad month is which of the three it was.
-- Folding a loss into a stock count would make every one of them look like a
-- counting mistake, and hide the pattern that says otherwise. Folding it into a
-- sale would put goods nobody paid for into the day's takings.
create table if not exists stock_correction (
    tenant_id      uuid   not null references tenant (id) on delete cascade,
    id             uuid   not null,
    item_id        uuid   not null,
    -- Signed: negative for goods gone, positive for a count that was under.
    qty_milli      bigint not null,
    -- Mandatory, and the schema says so. An unexplained correction is
    -- indistinguishable from theft when the variance is read a month later,
    -- which is the same reason a cash movement demands a reason.
    reason         text   not null,
    occurred_at_ms bigint not null,
    recorded_by    uuid   not null,
    recorded_at    timestamptz not null default now(),
    primary key (tenant_id, id),
    constraint stock_correction_reason_not_blank check (length(btrim(reason)) > 0)
);

create index if not exists stock_correction_by_item
    on stock_correction (tenant_id, item_id, occurred_at_ms);

alter table stock_correction enable row level security;
alter table stock_correction force row level security;

create policy tenant_isolation on stock_correction
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete on stock_correction to openpos_app;
    end if;
end
$$;
