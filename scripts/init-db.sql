-- The role the application connects as.
--
-- Not a superuser, on purpose: superusers bypass row-level security, so the
-- isolation policies would be inert at runtime while looking correct in the
-- schema. This is the single most common way multi-tenant RLS ends up doing
-- nothing at all.
create role openpos_app with login password 'openpos_app';
grant connect on database openpos to openpos_app;
