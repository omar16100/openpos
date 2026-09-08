-- The most the shop will let somebody owe at once.
--
-- A shop here sells on account all day. It could see what each person owed and
-- had no way to say "not past this": the only control was a cashier remembering
-- a number, at a counter, with the customer standing there. A shop whose cash is
-- on somebody else's shelf is the ordinary way a small one dies.
--
-- Poisha, like every other amount. Zero is no cap, which is what everybody has
-- until an owner sets one, so the column defaults to what the shop's position
-- already was.
alter table customer add column if not exists limit_minor bigint not null default 0;

comment on column customer.limit_minor is
    'The most this person may owe at once, in poisha. Zero is no cap';
