-- Terminal authentication.
--
-- A terminal proves who it is with a bearer token issued at enrolment. Only the
-- hash is stored: a stolen database gives an attacker no working credential.
--
-- SHA-256 rather than a password hash such as argon2, and the distinction
-- matters. Password hashes are deliberately slow because humans choose guessable
-- passwords. These tokens are 256 bits of randomness the server generates, so
-- there is nothing to guess, and a slow hash would only add latency to every
-- request a shop makes.
create table if not exists terminal_token (
    token_hash  bytea       primary key,
    tenant_id   uuid        not null,
    terminal_id uuid        not null,
    issued_at   timestamptz not null default now(),
    revoked_at  timestamptz
);

create index if not exists terminal_token_owner
    on terminal_token (tenant_id, terminal_id);

-- Deliberately not under row-level security, and this is the one exception in
-- the schema.
--
-- Every other table is filtered by the tenant the transaction is scoped to. This
-- table is what establishes which tenant that is, so it has to be readable
-- before the answer is known. It holds no business data: hashes, ids and
-- timestamps only. Putting it under RLS would create a circular dependency where
-- a request can never authenticate itself.
alter table terminal_token disable row level security;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'openpos_app') then
        grant select, insert, update on terminal_token to openpos_app;
    end if;
end
$$;
