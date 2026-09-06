-- What was decided about a sale, every time somebody decided it.
--
-- A resolution used to be final. That was deliberate: two people working the
-- same queue should not silently overwrite each other. But it also meant a
-- strike-out made in error could not be taken back, and a strike-out takes a
-- real debt off somebody's account. The entry had already left the queue, so
-- there was no screen it could be reached from either.
--
-- So a decision can be made again, and every one of them is kept. The sale's own
-- columns hold the latest, which is what the figures read; this table is who
-- said what and when, which is what anybody arguing about it needs. A shop that
-- struck out the wrong sale can say so, and the record shows both that it did
-- and that it changed its mind.
create table if not exists sale_resolution (
    tenant_id  uuid        not null references tenant (id) on delete cascade,
    sale_id    uuid        not null,
    -- Ordered by the server's own clock. Two decisions in the same millisecond
    -- would need a person pressing twice on two devices, and the seq breaks
    -- that tie so the latest is never ambiguous.
    seq        integer     not null,
    decided_at timestamptz not null default now(),
    note       text        not null,
    -- What was decided that time. Null is not allowed here: a row exists only
    -- because somebody answered.
    kept       boolean     not null,
    primary key (tenant_id, sale_id, seq)
);

alter table sale_resolution enable row level security;
alter table sale_resolution force row level security;

create policy tenant_isolation on sale_resolution
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

-- The recently decided list is read newest first, which is how somebody looking
-- for the mistake they just made finds it.
create index if not exists sale_resolved_recently on sale (tenant_id, resolved_at desc)
    where resolved_at is not null;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert on sale_resolution to openpos_app;
    end if;
end
$$;

-- Every decision already made becomes the first row of its own history, so the
-- list does not start empty for a shop that has been working its queue for
-- months.
--
-- Best effort, and nothing depends on it: `sale` has forced row level security,
-- so this reads nothing unless the role running migrations bypasses it. Where it
-- does nothing, the first change to an old decision writes that decision's own
-- row from the sale's columns before appending the new one, so the trail still
-- starts where the shop's record does.
insert into sale_resolution (tenant_id, sale_id, seq, decided_at, note, kept)
select tenant_id, id, 1, resolved_at, coalesce(resolution, ''),
       coalesce(resolution_kept, true)
  from sale
 where resolved_at is not null
on conflict do nothing;
