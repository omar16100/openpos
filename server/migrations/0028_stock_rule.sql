-- What a shop wants done when a basket asks for more than the shelf holds.
--
-- Three answers, because there is no one right one. A shop whose stock keeping
-- is good wants the till to refuse and make somebody say why. A shop whose
-- figures are roughly right wants to be told and to carry on serving the queue.
-- A shop that has never counted holds zero of everything as far as this system
-- knows, and a till that refused on that basis would be a till that cannot sell.
--
-- So it is the shop's decision and it starts at nothing. Turning it on is a
-- statement that the figures mean something, and that is not a statement this
-- software can make on a shop's behalf.
alter table tenant add column if not exists stock_rule smallint not null default 0;

comment on column tenant.stock_rule is
    'What a till does when a basket asks for more than the shelf holds: 0 nothing, 1 say so, 2 refuse and let a supervisor allow it';
