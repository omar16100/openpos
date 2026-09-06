-- Telling a stored catalogue payload which format it is in.
--
-- Every other stored payload in this system carries a schema number: the disk
-- frames do, the sync envelopes do, and the reason is written down in
-- `core::storage::wire` — postcard is positional, so adding one field to a
-- struct turns every previously written byte string into something that decodes
-- to nonsense or not at all.
--
-- `catalogue_change.payload` was the exception. It holds a postcard-encoded
-- `ItemWire` with nothing saying so. The day that struct gains a field, every
-- stored row for every shop stops decoding, `items_since` returns a backend
-- error, and the HTTP layer turns that into a 503: every till's pull loop, in
-- every shop, stops at once, and there is no way to read the old rows because
-- nothing recorded what they were.
--
-- One smallint now, against a migration that cannot be written later.
alter table catalogue_change
    add column if not exists schema smallint not null default 1;

-- The default covers every row written before this migration, all of which are
-- schema 1 by construction: there has only ever been one shape.
