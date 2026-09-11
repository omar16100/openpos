-- Which languages a shop offers its own staff.
--
-- This product speaks two, English and Bangla, and until now every device
-- offered both and each one remembered its own answer. That is right for a shop
-- whose people read both and wrong for the two shops either side of it: one
-- where nobody reads English and a cashier who presses the wrong button is
-- stranded in a language they cannot read their way out of, and one that works
-- in English and does not want a button on the till that can put a counter into
-- a script the person standing at it cannot use.
--
-- Empty means every language the device has, which is what every shop meant
-- before this column existed, so no shop is changed by its arrival.
--
-- A setting about the words this product chose, never about the words the shop
-- chose. A shop that reads English in the back office still sells goods whose
-- names are Bangla on the packet, and its catalogue, its search and what it
-- typed into its own records are untouched by this.
alter table tenant add column if not exists languages text[] not null default '{}';

comment on column tenant.languages is
    'The languages this shop offers its own staff, by the codes the screens use (en, bn). Empty means every language the device has';
