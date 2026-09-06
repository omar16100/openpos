-- Two things the account book was missing.
--
-- One: a payment was only unique per person, so one minted id could be counted
-- twice by sending it against two names. An id a client mints to stop a double
-- count has to be the thing that stops it, whoever it names.
create unique index if not exists account_entry_one_per_source
    on account_entry (tenant_id, source_id)
    where kind <> 1;

-- Two: nothing could take a debt off the book except money. A sale rung twice
-- on a till restored from a backup puts the same goods on somebody's account
-- twice, and until now the only way to correct it was to record a payment that
-- was never made, which is a lie in the one place a shop cannot afford one.
--
-- Kind 3 is a correction: negative like a payment, and told apart from one so
-- that money taken and money written off are never added together. A note is
-- required by the code that writes it, because a debt that vanishes without a
-- reason is the thing this book exists to prevent.
comment on column account_entry.kind is '1 sale on account, 2 payment taken, 3 written off';
