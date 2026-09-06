-- What a supervisor waived, and on which sale.
--
-- A cashier's ceiling is refused until somebody with authority allows it, and
-- what they allowed is written onto the ticket: it prints on the customer's
-- receipt and travels in the payload. That is the right place for it and the
-- wrong place to read it from: answering "what was waived this week, and by
-- whom" would mean decoding every ticket of the week.
--
-- So it is projected out as the sale arrives, like the tax rows beside it. One
-- row per waiver, in the order they were given, from the same bytes the
-- customer's paper was printed from.
create table if not exists sale_override (
    tenant_id uuid   not null references tenant (id) on delete cascade,
    sale_id   uuid   not null,
    -- Position on the ticket, so two waivers on one sale keep their order and a
    -- replay writes the same rows rather than a second set.
    seq       integer not null,
    reason    text   not null,
    primary key (tenant_id, sale_id, seq)
);

create index if not exists sale_override_by_sale on sale_override (tenant_id, sale_id);

alter table sale_override enable row level security;
alter table sale_override force row level security;

create policy tenant_isolation on sale_override
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on sale_override to openpos_app;
    end if;
end
$$;
