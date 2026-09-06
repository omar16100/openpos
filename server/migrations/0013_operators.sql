-- The people who stand at a till.
--
-- The core has known how to check a PIN and enforce a permission since it was
-- written, and nothing could create a person to check. A permission model with
-- no way to add somebody to it is not a permission model, it is a folder of
-- unreachable code.
--
-- The PIN is never here. What is stored is a PBKDF2 salt, a round count and a
-- derived key, computed on the owner's device by the same code the till uses to
-- verify, so the PIN itself does not cross the network and this table is worth
-- nothing to somebody who copies it. The round count is per row: raising the
-- cost later must not lock out everybody who set a PIN before.
create table if not exists operator (
    tenant_id  uuid   not null references tenant (id) on delete cascade,
    id         uuid   not null,
    name       text   not null,
    pin_salt   bytea  not null,
    pin_rounds integer not null,
    pin_key    bytea  not null,
    -- The permission flags, as the core models them. Stored as columns rather
    -- than a role name because a shop that wants a supervisor who cannot void
    -- sales should not have to wait for a release.
    max_discount_bp    integer not null default 0,
    may_override_price boolean not null default false,
    may_refund         boolean not null default false,
    may_void_line      boolean not null default false,
    may_authorise      boolean not null default false,
    may_open_drawer    boolean not null default false,
    may_close_shift    boolean not null default false,
    -- Cleared staff, or somebody suspended pending a conversation. Kept rather
    -- than deleted so their name still resolves on yesterday's tickets.
    active     boolean not null default true,
    created_at timestamptz not null default now(),
    primary key (tenant_id, id),
    constraint operator_name_not_blank check (length(btrim(name)) > 0),
    -- A zero round count would make the hash instant, which is the one property
    -- it must not have.
    constraint operator_rounds_sane check (pin_rounds >= 1000)
);

create index if not exists operator_by_shop on operator (tenant_id, active, name);

alter table operator enable row level security;
alter table operator force row level security;

create policy tenant_isolation on operator
    using (tenant_id = current_setting('openpos.tenant_id', true)::uuid)
    with check (tenant_id = current_setting('openpos.tenant_id', true)::uuid);

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete on operator to openpos_app;
    end if;
end
$$;
