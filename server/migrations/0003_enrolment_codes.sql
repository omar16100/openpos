-- Getting a credential onto a device.
--
-- A token is 64 hex characters. Nobody types that onto a tablet in a shop, and
-- sending it over WhatsApp is how credentials end up in message history forever.
-- So the back office issues a short code instead, and the device exchanges it
-- for a real token over the wire.
--
-- Three properties make a short code safe, and all three are required:
-- single use, a short expiry, and a hash rather than the code itself. Any one
-- of them missing and the code becomes a long-lived shared secret.
create table if not exists enrolment_code (
    code_hash   bytea       primary key,
    tenant_id   uuid        not null,
    terminal_id uuid        not null,
    expires_at  timestamptz not null,
    consumed_at timestamptz,
    created_at  timestamptz not null default now()
);

create index if not exists enrolment_code_expiry on enrolment_code (expires_at);

-- Read before the tenant is known, exactly like terminal_token, and for the
-- same reason: this is the request that establishes which tenant is calling.
-- Holds a hash, two identifiers and two timestamps.
alter table enrolment_code disable row level security;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update, delete on enrolment_code to openpos_app;
    end if;
end
$$;
