-- What each sale owed the revenue, by the rate it was charged at.
--
-- A shop here files a monthly return, and the figure it needs is what it sold at
-- each rate and what tax that came to. Until now the only place that lived was
-- inside the sale payloads: answering "what did we charge in VAT last month"
-- meant decoding every ticket of the month, which is the most expensive way to
-- answer a question a shop asks twelve times a year.
--
-- One row per sale per rate. Written from the server's own recomputation rather
-- than from anything the device stored, for the same reason the stock movements
-- are: what a shop declares to the revenue must not be something a payload could
-- assert.
create table if not exists sale_vat (
    tenant_id  uuid   not null references tenant (id) on delete cascade,
    sale_id    uuid   not null,
    -- Basis points, so 15 percent is 1500 and a rate that changes next year is
    -- a different row rather than a rewrite of this one.
    vat_bp     integer not null,
    net_minor  bigint not null,
    vat_minor  bigint not null,
    primary key (tenant_id, sale_id, vat_bp)
);

create index if not exists sale_vat_by_sale on sale_vat (tenant_id, sale_id);

alter table sale_vat enable row level security;
alter table sale_vat force row level security;

create policy tenant_isolation on sale_vat
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on sale_vat to openpos_app;
    end if;
end
$$;
