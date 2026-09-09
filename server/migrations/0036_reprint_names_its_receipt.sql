-- Which receipt a reprint was of.
--
-- A second copy of a receipt is a second piece of paper somebody can hand over:
-- an expense claimed twice, a return made against a sale already returned. The
-- trail has recorded that a reprint happened and who did it; the question a shop
-- asks afterwards is which one, and until now the answer was to line the times
-- up against the sales by hand.
--
-- Null for every other kind of entry, and for every reprint written before this
-- column existed. Those rows are the truth about themselves: the device that
-- wrote them did not carry a receipt number.
alter table allowed_action add column if not exists receipt_no text;

comment on column allowed_action.receipt_no is
    'The receipt a reprint was of. Null for every other action, and for reprints written before this column existed.';
