-- What the shop has paid its suppliers.
--
-- A shop here takes stock on credit and settles on a day of the week: the
-- distributor's man comes on Saturday and is paid for what came in since the
-- last Saturday. Deliveries have been recorded since purchasing existed, and
-- nothing was ever recorded against them, so what the shop owed its suppliers
-- lived on the same paper the customer book did.
--
-- Only the payments are stored. What is owed is the deliveries less these, and
-- both sides are already written down: a stored balance and a ledger that
-- disagree is a question nobody in a shop can answer, and it is the deliveries
-- that anybody would argue about anyway.
--
-- A delivery paid in cash at the door is a delivery and a payment of the same
-- amount on the same day, which is what the paper says too.
create table if not exists supplier_payment (
    tenant_id    uuid   not null references tenant (id) on delete cascade,
    -- Minted by whoever recorded the payment, so a resent one is not counted
    -- twice. Money the shop believes it has paid and has not is the same
    -- mistake as money it believes it was given.
    id           uuid   not null,
    supplier_id  uuid   not null,
    amount_minor bigint not null,
    paid_at_ms   bigint not null,
    note         text   not null default '',
    recorded_at  timestamptz not null default now(),
    primary key (tenant_id, id)
);

create index if not exists supplier_payment_by_supplier
    on supplier_payment (tenant_id, supplier_id, paid_at_ms desc);

alter table supplier_payment enable row level security;
alter table supplier_payment force row level security;

create policy tenant_isolation on supplier_payment
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on supplier_payment to openpos_app;
    end if;
end
$$;
