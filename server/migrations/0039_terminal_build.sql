-- Which build a device is running, as it last told the shop.
--
-- The support question a solo maintainer cannot answer without it: one till
-- behaves differently from the one beside it, and the first thing worth knowing
-- is whether they are running the same code. Until now the only way to find out
-- was to walk to each counter and look, which for a shop with a back office in
-- one room and tills in another is the difference between a phone call and a
-- journey.
--
-- The name is a hash of everything in the copy the device keeps of itself,
-- which is the only honest name a build has here: a version number would need
-- somebody to remember to change it, and the one that mattered would be the one
-- they forgot. Null for a device that has not said, which is every device until
-- it next syncs and any browser that refuses a service worker.
alter table terminal add column if not exists app_build text;

comment on column terminal.app_build is
    'The build the device last reported, a hash of the copy it keeps of itself. Null when it has not said';
