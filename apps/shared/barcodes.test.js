import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  SYMBOLOGIES,
  asThirteen,
  canReadBarcodes,
  checkDigitOf,
  checksOut,
  whatWasRead,
} from './barcodes.js';

test('the check digit is the one printed on the box', () => {
  // Real numbers off shelves this is for, and the last digit of each is what
  // the sum has to produce.
  for (const code of [
    '8901030865275', // a toothpaste sold across the subcontinent
    '4006381333931', // a German pen, which is the example everybody uses
    '8711200395743',
    '5901234123457',
  ]) {
    assert.equal(checkDigitOf(code.slice(0, -1)), Number(code[code.length - 1]), code);
    assert.equal(checksOut(code), true, code);
  }
});

test('a digit read wrong is caught by the digit that exists to catch it', () => {
  const right = '8901030865275';
  // Every single-digit misread of a real code, which is the failure a camera
  // held at an angle actually produces.
  let caught = 0;
  let missed = 0;
  for (let at = 0; at < right.length; at += 1) {
    for (let digit = 0; digit <= 9; digit += 1) {
      if (String(digit) === right[at]) continue;
      const wrong = `${right.slice(0, at)}${digit}${right.slice(at + 1)}`;
      if (checksOut(wrong)) missed += 1;
      else caught += 1;
    }
  }
  assert.equal(missed, 0, 'a single wrong digit always moves the check digit');
  assert.equal(caught, 117, 'thirteen places, nine other digits each');
});

test('a carton label checks out, and a misread one does not', () => {
  // ITF-14 on a wholesaler's outer, which is the same alternating sum as the
  // rest and was not being checked at all: fourteen digits went through
  // whatever they said.
  const carton = '10012345678902';
  assert.equal(checksOut(carton), true, carton);
  assert.equal(checksOut('10012345678903'), false, 'one digit out is caught');
  // Every single-digit misread of it, because that is what a camera produces.
  let missed = 0;
  for (let at = 0; at < carton.length; at += 1) {
    for (let digit = 0; digit <= 9; digit += 1) {
      if (String(digit) === carton[at]) continue;
      const wrong = `${carton.slice(0, at)}${digit}${carton.slice(at + 1)}`;
      if (checksOut(wrong)) missed += 1;
    }
  }
  assert.equal(missed, 0, 'a single wrong digit always moves the check digit');
});

test('a UPC-E off an imported tin is checked as what it stands for', () => {
  // The last digit printed on a UPC-E is the check digit of the twelve digits
  // it expands to, not of the eight as they stand. Checking it as an EAN-8
  // rejected real labels, and a shop whose imported tins would not scan had
  // nothing on the screen to say why.
  //
  // One of each of the six ways the zeros come out.
  for (const code of ['04252614', '01234505', '00567815', '01234133', '05012349', '04963503']) {
    assert.equal(checksOut(code), true, code);
  }
  // And a misread of one is still caught.
  assert.equal(checksOut('04252615'), false);
});

test('a code with no check digit of its own is taken as it was read', () => {
  // Code 128 and ITF carry no digit this can work out, and a shop's own carton
  // labels are exactly those. Refusing them would read as "the camera does not
  // work" and there would be nothing on the screen to explain it.
  assert.equal(checksOut('12345'), true);
  assert.equal(checksOut('CARTON-4471'), true);
  assert.equal(checksOut(''), true);
});

test('twelve digits and thirteen are the same tin', () => {
  // UPC-A off an import, and the catalogue holds it as thirteen. A shop that
  // files one and scans the other has two items where it has one.
  assert.equal(asThirteen('012345678905'), '0012345678905');
  assert.equal(asThirteen('8901030865275'), '8901030865275', 'thirteen is left alone');
  assert.equal(asThirteen('CARTON'), 'CARTON');
});

test('a camera has to read the same code twice before the till hears it', () => {
  const right = '8901030865275';
  // One frame is a guess. The second costs a fraction of a second, because a
  // camera gives twenty or thirty of them a second.
  const first = whatWasRead(null, right);
  assert.equal(first.ring, null, 'nothing is rung on one reading');
  assert.equal(first.seen, right);

  const again = whatWasRead(first.seen, right);
  assert.equal(again.ring, right, 'two readings that agree are a scan');
});

test('two readings that disagree ring nothing', () => {
  const first = whatWasRead(null, '8901030865275');
  const other = whatWasRead(first.seen, '4006381333931');
  assert.equal(other.ring, null, 'they disagree, so the camera keeps looking');
  assert.equal(other.seen, '4006381333931', 'and the newer one is what to agree with');
});

test('a reading that fails its own check digit is not remembered either', () => {
  // Not remembered on purpose: a misread held as "seen" would be rung the
  // moment the same misread happened twice, which is the one thing two
  // readings are supposed to prevent.
  const wrong = whatWasRead(null, '8901030865276');
  assert.equal(wrong.ring, null);
  assert.equal(wrong.seen, null);
  const twice = whatWasRead(wrong.seen, '8901030865276');
  assert.equal(twice.ring, null, 'a misread read twice is still a misread');
});

test('nothing read is nothing rung', () => {
  assert.deepEqual(whatWasRead(null, ''), { seen: null, ring: null });
  assert.deepEqual(whatWasRead('8901030865275', null), { seen: null, ring: null });
});

test('a browser without a reader says so rather than opening a dead window', () => {
  assert.equal(canReadBarcodes({}), false);
  assert.equal(canReadBarcodes({ BarcodeDetector: class {} }), true);
});

test('the symbologies are the ones a shop here has on its shelves', () => {
  // QR is deliberately absent: a QR code on a counter is a payment, not an
  // item, and reading one into the basket rings whatever number a customer's
  // phone happens to be showing.
  assert.ok(SYMBOLOGIES.includes('ean_13'));
  assert.ok(SYMBOLOGIES.includes('code_128'));
  assert.ok(!SYMBOLOGIES.includes('qr_code'));
});
