-- The role the application connects as.
--
-- Not a superuser, on purpose: superusers bypass row-level security, so the
-- isolation policies would be inert at runtime while looking correct in the
-- schema. This is the single most common way multi-tenant RLS ends up doing
-- nothing at all.
create role openpos_app with login password 'openpos_app';
grant connect on database openpos to openpos_app;

-- A second database, for the tests, and the reason is not speed.
--
-- A run of the suite leaves thousands of shops behind: every test that needs a
-- shop makes one, and nothing tidies up, which is right for a test and means
-- the database it ran against stops describing anything. Pointed at the same
-- database a demo shop lives in, it buries that shop: sixteen thousand tenants
-- and twenty thousand sales, of which one shop's two hundred and fifty were the
-- ones somebody wanted to look at. An hour was spent reading the wrong figure
-- off that table, which is the whole argument for this line.
--
-- Not for speed. Measured on both: the postgres-backed tests take the same half
-- second against a database with sixteen thousand shops in it as against an
-- empty one, and what makes a full run slow is compiling, not querying.
create database openpos_test;
grant connect on database openpos_test to openpos_app;
