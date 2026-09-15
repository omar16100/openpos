-- What each sale cost the shop, as its lines carried it.
--
-- A shop knows what it took. Until now nothing anywhere could say what it made,
-- which is the second question every owner asks and the one that decides what
-- to stock more of: a sack of rice that moves twice a day at four taka is worth
-- less shelf than soap that moves twice a week at forty.
--
-- Summed on the way in from the cost frozen onto each line by the till, never
-- from the item's cost today: a sack bought at 380 and sold at 430 made fifty
-- taka, and repricing that sale when the supplier puts the sack up would
-- rewrite a figure the owner already acted on.
alter table sale add column if not exists cost_minor bigint;

comment on column sale.cost_minor is
    'What the goods on this sale cost the shop, from the cost frozen on its lines. '
    'Null on sales stored before the shop computed it';

-- Whether every line on it carried a cost. A shop that has never entered what
-- it pays for anything would otherwise read a margin equal to its whole
-- turnover, believe it for a week, and then find out. False is the honest
-- answer and the report says how much of the period it covers.
alter table sale add column if not exists cost_known boolean;

comment on column sale.cost_known is
    'True when every line on the sale carried what the shop paid for it';
