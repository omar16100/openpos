-- A counter a till can ask about cheaply.
--
-- People, the shop's own details and who buys on account are re-fetched every
-- ten minutes, which is right for a cashier hired this morning and wrong for one
-- being locked out in a hurry: a person caught with their hand in the drawer
-- keeps ringing sales for ten minutes after the owner suspends them.
--
-- Fetching all three every half minute instead would be three large replies a
-- minute per till for data nobody touched. So this is bumped whenever any of
-- them changes, and a till asks for one number on the cadence it already pulls
-- the catalogue at. When the number has not moved, nothing else is asked for.
alter table tenant add column if not exists settings_seq bigint not null default 0;

comment on column tenant.settings_seq is
    'Bumped when the people, the shop or the account customers change';
