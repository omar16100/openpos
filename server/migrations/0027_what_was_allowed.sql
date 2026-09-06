-- Privileged actions a till allowed, and on whose authority.
--
-- The question asked afterwards is never "was this allowed" but "who allowed
-- it". A discount above a cashier's ceiling, a price typed over the catalogue's,
-- a refund, a line voided, the drawer opened outside a sale: each of those is a
-- moment somebody could be walking out with money, and each was recorded on the
-- device in memory and nowhere else. A tablet restarted, wiped or lost took the
-- answer with it.
--
-- The till keeps them beside its counted drawers, sends them, and drops them
-- only when the shop says it has them. Stored once per terminal and count: two
-- identical actions in one millisecond are possible and are two different
-- things, so the device's own counter is what tells them apart rather than the
-- clock.
create table if not exists allowed_action (
    tenant_id           uuid     not null references tenant (id) on delete cascade,
    terminal_id         uuid     not null,
    -- The device's own count of what it has allowed, ever.
    seq                 bigint   not null,
    -- The device's clock. Never trusted against another device's, and shown as
    -- what that till thought the time was, which is what the person standing at
    -- it saw.
    at_ms               bigint   not null,
    -- 1 discount, 2 price override, 3 refund, 4 void a line, 5 open the drawer,
    -- 6 close the drawer, 7 a PIN typed wrongly, 8 a PIN typed wrongly that
    -- locked that person out. A number rather than a word: these rows outlive
    -- the build that wrote them.
    --
    -- Seven and eight are not actions anybody was allowed to take. They are
    -- here because they belong in the same list for the person reading it: one
    -- wrong PIN is a fat thumb, six on a Thursday evening is something else,
    -- and only a shop looking at them beside the drawer openings can tell.
    action              smallint not null,
    -- Basis points, for a discount. Zero otherwise.
    bp                  integer  not null default 0,
    operator_id         uuid     not null,
    -- What they were called at the time. Copied rather than joined, for the
    -- reason a price on a line is copied: somebody since renamed, or gone from
    -- the shop, is still who this belongs to.
    operator_name       text     not null,
    -- Nil when nobody had to allow it: the operator's own permission covered
    -- it, which is a different fact from a supervisor standing at the counter.
    authorised_by       uuid     not null,
    authorised_by_name  text     not null,
    received_at         timestamptz not null default now(),
    -- The device's clock is in the key beside its count on purpose.
    --
    -- The count is bumped and written down in one step, and a device that dies
    -- between the two comes back having forgotten the bump: the next thing it
    -- allows takes a count the shop may already hold. Keyed on the count alone,
    -- that second record would be silently dropped as a duplicate, which is the
    -- one failure this table exists to prevent. Two different actions cannot
    -- share a millisecond as well as a count, because getting there needs a
    -- restart in between, so this keeps both. A genuine resend after a dropped
    -- reply matches on both and is stored once, which is what it should be.
    primary key (tenant_id, terminal_id, seq, at_ms)
);

-- Read by day, which is how an owner reads it: "what happened on Tuesday".
create index if not exists allowed_action_by_time on allowed_action (tenant_id, at_ms desc);

alter table allowed_action enable row level security;
alter table allowed_action force row level security;

create policy tenant_isolation on allowed_action
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on allowed_action to openpos_app;
    end if;
end
$$;
