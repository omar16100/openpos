import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { daysAgo, fromAnotherDay, thisMonth, today } from './days.js';
import { everyScreen } from './screens.js';

/// A moment that is one day in UTC and the next day where the shop is.
///
/// 2026-09-08 21:43 UTC. In Dhaka that is 2026-09-09 at a quarter to four in
/// the morning: the shop's day has turned over and Greenwich's has not.
const EARLY_MORNING = new Date('2026-09-08T21:43:30Z');

test('a shop’s day is the one it is trading in', () => {
  // Run with TZ=Asia/Dhaka, which is what the first shop this is for uses.
  // Without a timezone the assertion below would pass or fail depending on the
  // machine, which is the same class of bug it is testing for.
  if (new Date().getTimezoneOffset() !== -360) return;

  assert.equal(today(EARLY_MORNING), '2026-09-09');
  assert.notEqual(
    today(EARLY_MORNING),
    EARLY_MORNING.toISOString().slice(0, 10),
    'the UTC date is the day before, which is the whole defect',
  );
});

test('the date is the local one, whatever the machine', () => {
  // The property that holds everywhere: what comes back is what the device's
  // own calendar says, which is what the range those boxes describe is read
  // against. Asserted against the same clock rather than against a fixed
  // string, so this test says something on any machine.
  const at = EARLY_MORNING;
  assert.equal(
    today(at),
    `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, '0')}-${String(at.getDate()).padStart(2, '0')}`,
  );
  assert.match(today(at), /^\d{4}-\d{2}-\d{2}$/, 'and in the shape a date box wants');
});

test('single digits are padded, or a date box refuses the value', () => {
  const early = new Date(2026, 0, 5, 9, 0, 0);
  assert.equal(today(early), '2026-01-05');
  assert.equal(thisMonth(early), '2026-01');
});

test('a week ago is seven days, not a hundred and sixty-eight hours', () => {
  // Counted by setting the date rather than subtracting milliseconds. A shop
  // that puts its clocks back would otherwise get a window starting an hour
  // late once a year, which is an hour of a trading day quietly outside it.
  const at = new Date(2026, 8, 9, 5, 43, 0);
  assert.equal(daysAgo(7, at), '2026-09-02');
  assert.equal(daysAgo(0, at), today(at));

  // Across a month boundary, and across a year.
  assert.equal(daysAgo(9, new Date(2026, 8, 3, 12, 0, 0)), '2026-08-25');
  assert.equal(daysAgo(3, new Date(2026, 0, 1, 12, 0, 0)), '2025-12-29');
});

test('no screen fills a date box with the date in Greenwich', () => {
  // The defect itself, as a property of the source. `toISOString()` gives the
  // UTC date, and every date box in the back office describes a range read from
  // local midnight to local midnight: east of Greenwich the two disagree for
  // the first hours of every morning, and Bangladesh is six hours east.
  //
  // A shopkeeper in Dhaka at five in the morning was handed a window that ended
  // at midnight the night before, so the trail said nothing had happened, and
  // the day report showed yesterday's takings under today's heading. Both are
  // silent: nothing on either screen says which day it is looking at.
  // Every file of both screens, not the two that happened to hold them when
  // this was written: a screen split into panels is a screen this would
  // otherwise go on passing about while reading half of it.
  for (const { path, source } of everyScreen()) {
    assert.equal(
      source.includes('toISOString'),
      false,
      `${path} builds a date from toISOString, which is the day in Greenwich and not the day ` +
        `the shop is trading in. Use today(), daysAgo() or thisMonth() from days.js.`,
    );
  }
});

test('a drawer opened today is not from another day', () => {
  const now = new Date('2026-09-13T08:30:00');
  assert.equal(fromAnotherDay(new Date('2026-09-13T06:05:00').getTime(), now), false);
  assert.equal(fromAnotherDay(new Date('2026-09-13T23:59:59').getTime(), now), false);
  assert.equal(fromAnotherDay(new Date('2026-09-13T00:00:00').getTime(), now), false);
});

test('a drawer nobody closed last night is', () => {
  // The morning this is for: a cashier reads "7 sales, should hold 1,500.40"
  // and those seven are yesterday's.
  const now = new Date('2026-09-13T08:30:00');
  assert.equal(fromAnotherDay(new Date('2026-09-12T21:40:00').getTime(), now), true);
  assert.equal(fromAnotherDay(new Date('2026-09-09T16:59:49').getTime(), now), true);
});

test('an hour either side of midnight is two different days', () => {
  // And not "less than a day ago", which is the rule somebody reaches for and
  // which would say nothing on exactly the morning it is wanted.
  const now = new Date('2026-09-13T00:30:00');
  assert.equal(fromAnotherDay(new Date('2026-09-12T23:30:00').getTime(), now), true);
});

test('a drawer with no opening time says nothing', () => {
  // A build that did not keep one. "Open since 1970" is worse than silence.
  const now = new Date('2026-09-13T08:30:00');
  assert.equal(fromAnotherDay(0, now), false);
  assert.equal(fromAnotherDay(undefined, now), false);
  assert.equal(fromAnotherDay(null, now), false);
  assert.equal(fromAnotherDay(-1, now), false);
});
