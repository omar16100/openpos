-- Whether a sale somebody looked at is a sale.
--
-- The repair queue has always taken a note and nothing else. That is enough for
-- "the totals disagreed by one poisha, left as it stands" and not enough for the
-- case it exists for: a till restored from a backup rang the same goods twice,
-- and one of those two sales did not happen.
--
-- Until now the shop could write that down and the figures ignored it. The
-- duplicate stayed in the day's takings, in the month's tax, in what sold, and
-- in the shelf figures, for ever. A note nobody's arithmetic reads is a note
-- that only makes somebody feel better.
--
-- Null means nobody has decided. True means it stands. False means it was not a
-- sale, and everything that counted it stops counting it: the money, the tax,
-- what left the shelf, and anything it put on somebody's account. Nothing is
-- deleted: the sale, its bytes, its movements and its account entries all stay
-- where they were, and the figures filter rather than write anything back.
--
-- Decided once. A second person working the same queue is told nothing moved
-- rather than quietly overwriting the first one's answer. That also means a
-- strike-out made in error cannot yet be taken back, which is written down in
-- todo.md rather than papered over here.
alter table sale add column if not exists resolution_kept boolean;

comment on column sale.resolution_kept is
    'Null until somebody decides. False means it was not a sale and nothing counts it';

-- The figures all filter on this, so it belongs in the index they use.
create index if not exists sale_struck_out on sale (tenant_id, resolution_kept)
    where resolution_kept is false;
