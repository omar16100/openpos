-- Making "this receipt number is already used" a fact the database decides,
-- rather than a question the application asks and then acts on.
--
-- Before this, ingest read the sale table to see whether a number was taken and
-- then, in a separate transaction, stored the sale. Two pushes carrying the same
-- number at the same moment both read "free" and both stored clean. That is not
-- a theoretical interleaving: the case the check exists for is a tablet restored
-- from a backup, and a restored tablet pushes its whole backlog at once, beside
-- the original device doing the same. The one moment the check matters is the
-- one moment it failed.
--
-- A unique index on `sale` was the obvious fix and the wrong one. The second
-- sale would be refused outright, and refusing it is exactly what the design
-- rejects everywhere else: the goods left the shop and the money changed hands,
-- so the server records it and raises a repair item. A claim in its own table
-- lets the insert of the sale always succeed while the claim of the number can
-- fail, which is the shape the product needs.
create table if not exists receipt_claim (
    tenant_id     uuid   not null references tenant (id) on delete cascade,
    -- The epoch is part of the key. A terminal the server believes was replaced
    -- gets a new epoch, and numbers reissued under it are a different series,
    -- attributable rather than colliding.
    receipt_epoch bigint not null,
    receipt_no    text   not null,
    -- Which sale holds it. Kept so the repair queue can name the other sale
    -- rather than telling a shopkeeper only that something is wrong.
    sale_id       uuid   not null,
    claimed_at    timestamptz not null default now(),
    primary key (tenant_id, receipt_epoch, receipt_no)
);

alter table receipt_claim enable row level security;
alter table receipt_claim force row level security;

create policy tenant_isolation on receipt_claim
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

-- Backfill from the sales already stored. Duplicates that predate this table
-- resolve to whichever row the scan reaches first, which is the best that can be
-- said retrospectively; from here on the primary key decides.
insert into receipt_claim (tenant_id, receipt_epoch, receipt_no, sale_id)
select distinct on (tenant_id, receipt_epoch, receipt_no)
       tenant_id, receipt_epoch, receipt_no, id
from sale
where receipt_no is not null and receipt_epoch is not null
order by tenant_id, receipt_epoch, receipt_no, received_at
on conflict do nothing;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete on receipt_claim to openpos_app;
    end if;
end
$$;
