-- Why a sale is held, as the reason itself rather than only as a sentence.
--
-- The queue is read by an owner deciding whether a sale is real, and until now
-- the only record of why it was held was English prose. A shop whose screens
-- read Bangla was handed one English paragraph at the one moment it is being
-- asked to make a judgement, and no screen could say it any other way: there
-- was nothing to translate against.
--
-- The reason travels as postcard, the same encoding every other shape in this
-- system uses, so the screen can be handed a name for what happened and the
-- figures beside it. The sentence stays where it is and stays authoritative for
-- every row written before this: a reason nobody can decode is still a reason
-- somebody can read.
alter table sale add column if not exists quarantine_kind bytea;

comment on column sale.quarantine_kind is
    'The QuarantineReason as postcard, for a screen wording it in the shop''s language. Null for rows written before the column existed, and for sales nobody held.';
