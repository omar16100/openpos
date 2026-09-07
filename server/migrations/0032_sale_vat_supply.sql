-- Which kind of supply each tax row is.
--
-- A rate of zero is not one thing. A zero-rated supply is taxable at nothing
-- and an exempt supply is outside the tax, and a return declares them in
-- different places: one carries a credit for the tax the shop paid on its own
-- inputs and the other does not. Until now a shop that had to tell them apart
-- on a return could not tell them apart here, because both arrived as a rate
-- of zero.
--
-- Which goods are which is the revenue's word and the shop's to set. Nothing
-- here decides it; this only keeps the answer once somebody has given it.
--
-- 0 standard rated, 1 zero rated, 2 exempt. Zero as the default because every
-- row written before this was charged at whatever rate it carried, which is
-- the standard treatment, and reading old rows as exempt would move turnover
-- out of a return that has already been filed.
alter table sale_vat add column if not exists supply smallint not null default 0;

comment on column sale_vat.supply is
    '0 standard rated, 1 zero rated, 2 exempt';

-- The key has to carry it. Two rows of one sale can both be at a rate of zero
-- and belong in different places, and the old key would have thrown the second
-- one away as a duplicate: a shop selling something zero rated beside
-- something exempt would have declared only whichever arrived first.
alter table sale_vat drop constraint if exists sale_vat_pkey;
alter table sale_vat add primary key (tenant_id, sale_id, vat_bp, supply);
