-- Giving a terminal credential an end, and a trail.
--
-- A token issued at enrolment worked forever. Nothing about that is safe: a
-- tablet sold on, lost in a rickshaw, or handed back by a departing employee
-- keeps a working credential for that shop until somebody notices and revokes
-- it, and the shops this product is for do not have somebody whose job that is.
--
-- Two columns. An expiry, so a credential that nobody thinks about stops
-- working on its own. And a last-used timestamp, so "this token was used from
-- two places today" becomes a question the back office can answer rather than a
-- thing nobody could have known.

-- Null means no expiry, which is every token issued before this migration.
-- Deliberately not backfilled to a date: expiring every shop's live terminals
-- at deploy time would take every till in the product offline at once, and the
-- till that goes offline is the one in the middle of a sale. New tokens get an
-- expiry; old ones keep working until they are re-enrolled.
alter table terminal_token add column if not exists expires_at timestamptz;

-- Null until the token is used. `issued_at` says a credential was created, not
-- that anything ever presented it, and the difference is the whole of a
-- "should this still exist" conversation.
alter table terminal_token add column if not exists last_used_at timestamptz;

-- Authentication reads by hash, which is the primary key, so no index is needed
-- for the check itself. This one serves the back office listing a terminal's
-- credentials oldest-used first, which is how a stale one is spotted.
create index if not exists terminal_token_last_used
    on terminal_token (tenant_id, terminal_id, last_used_at);
