-- What a credential is allowed to be used for.
--
-- Until now every enrolled device could do everything: ring sales, and also read
-- the shop's repair queue, edit its prices, book deliveries and record stock
-- counts. A shop with six tills had six devices that could reprice the whole
-- catalogue, and any one of them left on a counter was the whole shop.
--
-- Two roles, because two is what the shops this is for actually have. A till
-- rings sales and syncs. An owner does that and the back office as well. Finer
-- permissions belong to the person signing in at the till, which `core::auth`
-- already handles; this is about what the *device* may be used for, which is a
-- different question and needs a different answer.
alter table terminal_token  add column if not exists role smallint not null default 1;
alter table enrolment_code  add column if not exists role smallint not null default 1;

comment on column terminal_token.role is '1 till, 2 owner';
comment on column enrolment_code.role is 'the role the credential this code issues will carry';

-- Existing credentials become owners, and this is deliberate in both directions.
--
-- Downgrading them to tills would take away an ability they demonstrably had:
-- before this migration every credential could reach the back office, so that is
-- what they were, and a migration that quietly removes access leaves a shop
-- unable to fix its own prices with nothing saying why. Leaving them as tills
-- would also leave a tenant with no owner credential at all and no way to mint
-- one, since issuing an owner code requires being an owner.
--
-- New enrolments default to till, which is where the tightening actually
-- happens.
update terminal_token set role = 2;

create index if not exists terminal_token_by_role
    on terminal_token (tenant_id, role)
    where revoked_at is null;
