-- Which schema a stored sale's bytes were written under.
--
-- The till has always said, in the envelope it pushes. The shop wrote the bytes
-- down and threw the schema away, so everything that reads a payload back had
-- to guess: try each decoder this build knows, newest first, and take the one
-- that parses.
--
-- postcard is positional and has no tags, so that guess is not safe. The
-- current sale line is the older line with a cost appended, which means an
-- older payload offered to the newer reader can parse: it takes whatever
-- follows the line as the cost, everything after it shifts, and nothing errors.
--
-- Found by restoring a real shop's backup and comparing it against the shop.
-- One sale came back declaring no tax at all. It had declared 430.00 at fifteen
-- percent, the restore declared nothing, and the total still read 494.50 either
-- way, so no check anywhere noticed. The payload was a version two sale that
-- also parses as version four.
--
-- Null for every sale stored before this, which is every sale in every shop
-- today. Those cannot be recovered from the bytes; what they can be recovered
-- from is the shop's own record of what it declared, which is what the export
-- does with them.
alter table sale add column if not exists payload_schema smallint;

comment on column sale.payload_schema is
    'The schema the till said these bytes were written under; null for sales stored before it was kept';
