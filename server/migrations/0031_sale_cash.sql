-- What each sale actually put in a drawer.
--
-- The one figure an owner acts on is the variance on a counted drawer, and
-- until now the whole of it was the till's word: the till said what it expected,
-- somebody counted, and the shop stored both without ever asking whether its own
-- sales came to that. A till reporting a lower expectation hides a shortfall,
-- and nothing could see it.
--
-- Cash tenders less the change handed back, which is what stayed in the drawer.
-- A card, a wallet and a sale on account are money the shop is owed or has been
-- paid by other means: real, and not in the till.
--
-- Computed on the way in from the same crate that priced the sale, never read
-- from the payload's own assertion, for the reason the tax rows and the stock
-- movements are.
-- Nullable, and no default, because a sale stored before this existed was never
-- asked the question. Zero would say it put nothing in the drawer, and a drawer
-- of those sales would come to the float alone: a shop reading its own history
-- would be told every evening of it disagreed with the till. Null says nobody
-- worked it out, and the shop declines to answer for those drawers instead.
alter table sale add column if not exists cash_minor bigint;

comment on column sale.cash_minor is
    'What this sale left in the drawer: cash tenders less change given back. '
    'Null on sales stored before the shop computed it';

-- The question is always "this till, between these two times".
create index if not exists sale_drawer_window
    on sale (tenant_id, terminal_id, rung_at_ms);
