-- The receipt a refund reverses, out where the shop can ask about it.
--
-- It has always been inside the sale's own bytes, which is the right place for
-- the record and the wrong place for a question: nothing could ask "how much has
-- been refunded against this receipt" without decoding every sale in the shop.
-- So the same fact is written beside the sale as it arrives.
--
-- What it is for: a refund against a receipt this shop does not have, and a
-- receipt refunded for more than it was ever rung for. Neither is refused when
-- it arrives, because the goods came back and the money went out; both are held
-- for somebody to look at, which is what this system does with anything only a
-- person can settle.
alter table sale add column if not exists refund_of text;

comment on column sale.refund_of is
    'For a refund, the receipt number it reverses, as the till wrote it';

-- Answering "what has been refunded against this receipt" without reading the
-- shop's whole ledger.
create index if not exists sale_refund_of
    on sale (tenant_id, refund_of)
    where refund_of is not null;
