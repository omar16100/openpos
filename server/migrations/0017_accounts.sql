-- What people owe the shop, and what they have paid off it.
--
-- A shop here sells on account all day: a regular takes rice now and settles on
-- Friday. The till has been able to take "on account" and to name who took it,
-- and neither of those added up: nothing said what one person owed or let
-- anybody mark it paid, so the book stayed on paper beside the till.
--
-- One row per person per source, which is what makes a replayed sale and a
-- resent payment cost nothing. A balance is the sum of these and is never
-- stored: a stored balance and a ledger that disagree is a question nobody in a
-- shop can answer.
create table if not exists account_entry (
    tenant_id   uuid   not null references tenant (id) on delete cascade,
    -- The name folded for adding up: case and spacing do not make two people.
    person_key  text   not null,
    -- The name as the cashier wrote it, for showing back.
    person_name text   not null,
    -- What put this here: a sale on account, or a payment against one.
    source_id   uuid   not null,
    -- 1 sale on account, 2 payment taken.
    kind        smallint not null,
    -- Positive is owed to the shop. A payment and a refund are both negative,
    -- which is the same arithmetic from the other side.
    amount_minor bigint not null,
    at_ms       bigint not null,
    note        text   not null default '',
    received_at timestamptz not null default now(),
    primary key (tenant_id, source_id, person_key)
);

create index if not exists account_entry_by_person
    on account_entry (tenant_id, person_key, at_ms desc);

alter table account_entry enable row level security;
alter table account_entry force row level security;

create policy tenant_isolation on account_entry
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on account_entry to openpos_app;
    end if;
end
$$;
