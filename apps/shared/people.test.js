import test from 'node:test';
import assert from 'node:assert/strict';

import { fold, label, nameTaken, shared } from './people.js';

const karim = { id: '01J0000000000000000007QF3', name: 'Karim', active: true };
// Typed by a different person on a different day, which is how it happens.
const otherKarim = { id: '01J000000000000000000AB12', name: 'karim', active: true };
const rahima = { id: '01J000000000000000000ZZZ9', name: 'Rahima', active: true };

test('a name is compared the way two people saying it would', () => {
  assert.equal(fold(' Karim '), 'karim');
  assert.equal(fold('KARIM  Uddin'), 'karim uddin');
  assert.equal(fold(null), '');
});

test('a shop with one Karim never sees an identifier', () => {
  const twice = shared([karim, rahima]);
  assert.equal(twice.size, 0);
  assert.equal(label(karim, twice), 'Karim');
});

test('a shop with two Karims gets buttons that differ', () => {
  const twice = shared([karim, otherKarim, rahima]);
  assert.deepEqual([...twice], ['karim']);
  assert.equal(label(karim, twice), 'Karim · 7QF3');
  assert.equal(label(otherKarim, twice), 'karim · AB12');
  assert.equal(label(rahima, twice), 'Rahima', 'and everybody else is left alone');
  assert.notEqual(label(karim, twice), label(otherKarim, twice));
});

test('somebody retired does not mark a live person', () => {
  // They are not standing at the counter, so putting an identifier on the
  // person who is would be noise for nothing.
  const gone = { id: '01J000000000000000000GONE', name: 'Karim', active: false };
  const twice = shared([karim, gone]);
  assert.equal(twice.size, 0);
  assert.equal(label(karim, twice), 'Karim');
});

test('the same rules serve the people who buy on account', () => {
  // The records are the same shape and the cost of confusing two of them is
  // higher: what one took goes on one account and what they paid on the other,
  // and neither balance is theirs.
  const karim = { id: '01J0000000000000000007QF3', name: 'Karim, flat 3', active: true };
  const other = { id: '01J000000000000000000AB12', name: 'karim, flat 3', active: true };
  const twice = shared([karim, other]);
  assert.equal(label(karim, twice), 'Karim, flat 3 · 7QF3');
  assert.equal(nameTaken([karim], 'KARIM,  flat 3'), true);
  // Correcting their own record is not a clash with themselves.
  assert.equal(nameTaken([karim], 'Karim, flat 3', karim.id), false);
});

test('a name already in use is reported before a second one is added', () => {
  assert.equal(nameTaken([karim, rahima], 'karim'), true);
  assert.equal(nameTaken([karim, rahima], ' KARIM '), true);
  assert.equal(nameTaken([karim, rahima], 'Karim Uddin'), false);
  // Correcting somebody's own record is not a clash with themselves.
  assert.equal(nameTaken([karim, rahima], 'Karim', karim.id), false);
  // Somebody retired is not in the way of the name either.
  const gone = { id: 'x', name: 'Rahima', active: false };
  assert.equal(nameTaken([gone], 'Rahima'), false);
});

/// A list read from this device's own copy is re-read when that copy changes.
///
/// The back office draws its people from the device's own store, and a device
/// enrolled a minute ago has no people in it: they arrive on a round of their
/// own, up to ten minutes later. Nothing re-read the list when they did, so a
/// back office opened on a replacement tablet sat under the sentence that
/// belongs to a shop with nobody in it while the shop had five people, and went
/// on doing so for as long as the tab stayed open. Walked, and then walked again
/// after a reload, where all five appeared: the store had them the whole time.
///
/// Guarded here rather than in the screen tests because what it is really about
/// is people: the check that stops a shop having two people of one name reads
/// this same list, so an empty list warns about nothing and every name typed
/// that morning becomes a second copy of somebody already there.
test('the back office re-reads the people when the round that fetches them lands', async () => {
  const { screenOf } = await import('./screens.js');
  const admin = screenOf('admin');
  assert.ok(admin.length > 0, 'the back office is where it was');

  const asked = admin.filter((file) => /did\s*===\s*'operators'/.test(file.source));
  assert.equal(
    asked.length,
    1,
    'exactly one place notices the round that brings the people back'
  );
  const after = asked[0].source.slice(asked[0].source.search(/did\s*===\s*'operators'/));
  assert.match(
    after.slice(0, 200),
    /listPeople\(/,
    'and what it does about it is read the list again'
  );
});
