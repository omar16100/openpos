-- Who counted the drawer.
--
-- A variance attached to a till and a time is half of what an owner wants to
-- know. The name is stored beside the id rather than joined at read time,
-- because somebody who has since left the shop, or been renamed, is still the
-- person this particular count belongs to.
alter table closed_shift add column if not exists closed_by uuid;
alter table closed_shift add column if not exists closed_by_name text not null default '';

comment on column closed_shift.closed_by_name is 'What they were called when they counted it';
