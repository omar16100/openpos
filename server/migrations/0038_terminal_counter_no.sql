-- Which counter this is, in the shop's own words: 1, 2, 3.
--
-- The number printed on a receipt was made from the terminal's identifier, the
-- low sixteen bits of it in hex, because it had to be short enough to read
-- aloud over the phone. Two terminals whose identifiers happen to share those
-- bits therefore printed the same prefix, and their receipt counters are their
-- own, so both printed T7-000001. The clash is caught when the second device
-- syncs, which is after a customer is holding the paper, and what the shop then
-- has is two sales with one receipt number and a duplicate in its repair queue.
-- One pair in sixty five thousand is rare in a shop and certain across enough
-- of them.
--
-- A number the shop hands out cannot collide, and it is also what a shopkeeper
-- already says: counter one, counter two. It comes from a sequence on the
-- tenant rather than from `max(counter_no) + 1`, so two people adding a till at
-- the same moment cannot be given the same number.
alter table tenant add column if not exists terminal_seq bigint not null default 0;
alter table terminal add column if not exists counter_no integer not null default 0;

-- Everything already registered, in the order it was enrolled, so a shop's
-- oldest counter is counter one.
with numbered as (
    select tenant_id, id,
           row_number() over (partition by tenant_id order by enrolled_at, id) as n
      from terminal
)
update terminal t
   set counter_no = numbered.n
  from numbered
 where numbered.tenant_id = t.tenant_id and numbered.id = t.id
   and t.counter_no = 0;

update tenant
   set terminal_seq = coalesce(
       (select max(counter_no) from terminal where terminal.tenant_id = tenant.id), 0);

-- A receipt number is claimed per shop, per epoch, so numbers printed under the
-- old prefixes have to stay in an epoch of their own. Every terminal's epoch
-- moves on, which is the same act as a device being wiped and starting its
-- numbering again, and the screen that shows a shop where its numbering jumps
-- already reads a series per epoch. Without this, a terminal newly called
-- counter two could print a number another terminal had already claimed under
-- the prefix it happened to have.
update terminal set epoch = epoch + 1;

create unique index if not exists terminal_counter_no
    on terminal (tenant_id, counter_no)
 where counter_no > 0;

comment on column terminal.counter_no is
    'Which counter this is in its shop, 1 upward, and what a receipt number is prefixed with';
