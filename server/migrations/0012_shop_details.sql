-- What goes at the top of a receipt.
--
-- A till prints with the internet down, so these have to be on the device
-- before they are needed, which means the server has to hold them and hand them
-- over. Keeping them on each tablet instead would mean typing a BIN into every
-- one, and getting it wrong on the sixth.
--
-- Nullable throughout, and the receipt omits what is missing rather than
-- printing an empty label. A shop trading before its BIN comes through is an
-- ordinary shop, not a broken record.
alter table tenant add column if not exists bin     text;
alter table tenant add column if not exists address text;
alter table tenant add column if not exists phone   text;

comment on column tenant.bin is 'Business Identification Number, printed on receipts';
