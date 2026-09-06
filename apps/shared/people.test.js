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
