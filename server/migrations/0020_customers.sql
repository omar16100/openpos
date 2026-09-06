-- The people a shop lets buy on account.
--
-- The account book has been keyed on the name a cashier typed, folded so that
-- spacing and case do not split one person in three. That is what the paper
-- notebook beside the till already did, and it has the notebook's weakness: two
-- Karims share an account unless somebody remembers to write the flat number
-- every time.
--
-- So a shop can write them down. A sale that names one of these lands on that
-- person whatever the cashier typed, and the typed spelling is still what is
-- shown back, because it is what is on the receipt in somebody's hand.
--
-- Held on tills as well, like the people who may sign in: a sale on account is
-- written with the internet down.
create table if not exists customer (
    tenant_id  uuid    not null references tenant (id) on delete cascade,
    id         uuid    not null,
    name       text    not null,
    -- How the shop chases them. Optional: a shop that knows exactly who
    -- "Karim, flat 3" is should not be stopped by a form.
    phone      text,
    -- False when the shop has stopped letting them buy on account. Kept rather
    -- than deleted: what they already owe does not stop being owed, and their
    -- name is on entries in the book.
    active     boolean not null default true,
    updated_at timestamptz not null default now(),
    primary key (tenant_id, id)
);

create index if not exists customer_by_name on customer (tenant_id, name);

alter table customer enable row level security;
alter table customer force row level security;

create policy tenant_isolation on customer
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update on customer to openpos_app;
    end if;
end
$$;
