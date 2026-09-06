-- Stock that arrives, and where it came from.
--
-- Until now the only way stock moved was a sale, so the movement ledger was
-- keyed on a sale id and its timing was read by joining back to the sale row.
-- Goods arriving from a supplier are a movement with no sale behind them, so
-- both assumptions have to go.

-- A movement's source is a sale, a goods receipt, or a correction. The column
-- keeps its meaning rather than a sale id column quietly holding receipt ids,
-- which is the kind of lie that survives into every query written afterwards.
alter table stock_movement rename column sale_id to source_id;
alter table stock_movement add column if not exists source_kind smallint not null default 1;

comment on column stock_movement.source_kind is '1 sale, 2 goods receipt, 3 correction';

-- The two times a movement needs, carried on the movement itself.
--
-- Stock counts are barriers, and deciding which side of a barrier a movement
-- falls on takes both when it happened and when the server learned of it. Those
-- lived on the sale row, which meant on-hand joined every movement back to a
-- sale, and meant a movement with no sale had nowhere to keep them.
alter table stock_movement add column if not exists occurred_at_ms bigint;
alter table stock_movement add column if not exists recorded_at timestamptz;

update stock_movement m
   set occurred_at_ms = s.rung_at_ms,
       recorded_at    = s.received_at
  from sale s
 where s.tenant_id = m.tenant_id
   and s.id = m.source_id
   and m.occurred_at_ms is null;

-- Any row the backfill could not reach has no sale behind it, which before this
-- migration was impossible. Zero rather than null so the columns can be
-- required from here on: a movement with no time is a movement no barrier can
-- place, and one that sorts before every count is the safe direction.
update stock_movement set occurred_at_ms = 0 where occurred_at_ms is null;
update stock_movement set recorded_at = now() where recorded_at is null;

alter table stock_movement alter column occurred_at_ms set not null;
alter table stock_movement alter column recorded_at set not null;
alter table stock_movement alter column recorded_at set default now();

-- On-hand reads a shop's movements for one item across a barrier. With the
-- times on the row this is one index and no join.
create index if not exists stock_movement_by_item
    on stock_movement (tenant_id, item_id, occurred_at_ms);

-- Who the shop buys from.
create table if not exists supplier (
    tenant_id  uuid not null references tenant (id) on delete cascade,
    id         uuid not null,
    name       text not null,
    phone      text,
    -- Business Identification Number. Nullable because most small suppliers in
    -- a neighbourhood market do not have one, and a schema that insists would
    -- be worked around by typing zeros.
    bin        text,
    active     boolean not null default true,
    created_at timestamptz not null default now(),
    primary key (tenant_id, id)
);

-- Goods arriving. One receipt, many lines, and the lines are what move stock.
create table if not exists goods_receipt (
    tenant_id      uuid not null references tenant (id) on delete cascade,
    id             uuid not null,
    supplier_id    uuid,
    -- The supplier's own invoice or challan number, which is what a shopkeeper
    -- has in their hand when querying a delivery.
    reference      text,
    -- When the goods arrived, by the clock of whoever recorded it. Decides which
    -- side of a stock count the arrival falls on.
    received_at_ms bigint not null,
    received_by    uuid not null,
    note           text,
    recorded_at    timestamptz not null default now(),
    primary key (tenant_id, id)
);

create table if not exists goods_receipt_line (
    tenant_id  uuid   not null,
    receipt_id uuid   not null,
    item_id    uuid   not null,
    qty_milli  bigint not null,
    -- What this delivery cost per unit, in minor units. Kept per line and per
    -- delivery rather than only on the item, because the price a shop paid last
    -- Tuesday is what a margin is actually measured against.
    unit_cost_minor bigint not null,
    primary key (tenant_id, receipt_id, item_id),
    foreign key (tenant_id, receipt_id) references goods_receipt (tenant_id, id) on delete cascade
);

alter table supplier            enable row level security;
alter table goods_receipt       enable row level security;
alter table goods_receipt_line  enable row level security;

alter table supplier            force row level security;
alter table goods_receipt       force row level security;
alter table goods_receipt_line  force row level security;

create policy tenant_isolation on supplier
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);
create policy tenant_isolation on goods_receipt
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);
create policy tenant_isolation on goods_receipt_line
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

create index if not exists goods_receipt_by_supplier
    on goods_receipt (tenant_id, supplier_id, received_at_ms);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete
            on supplier, goods_receipt, goods_receipt_line to openpos_app;
    end if;
end
$$;
