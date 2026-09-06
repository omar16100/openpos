-- The wallets a shop takes, by the name a report should read.
--
-- Set once by the owner rather than typed at a till on every sale. A shop that
-- takes bKash and Nagad would otherwise type both names all day, and one typo
-- makes a third wallet that gets its own line in every report and reconciles
-- against nothing anybody can find.
--
-- A list on the shop rather than a table of its own: this is a handful of short
-- strings a shop changes twice a year, and a table would be a join on every
-- receipt lookup for no gain.
alter table tenant add column if not exists wallets text[] not null default '{}';

comment on column tenant.wallets is 'Mobile wallets the shop accepts, by name';
