-- The buyer's own Business Identification Number.
--
-- A tax invoice in this country names the supplier's BIN and the buyer's, and
-- the buyer's has nowhere to live until here. It costs nothing to carry now and
-- cannot be filled in retrospectively for sales already made: a shop that sells
-- to another business needs it on the paper on the day.
--
-- Optional, because most buyers in a general store are people rather than
-- businesses and a form that insisted would be a form nobody fills in.
alter table customer add column if not exists bin text;

comment on column customer.bin is
    'The buyer''s Business Identification Number, for a sale to another business';
