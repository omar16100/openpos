import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { against, readCatalogue, tooEarlyToMatch, whatWillBeWritten } from './catalogue_file.js';

test('a device that has not read the shop may not match a file against it', () => {
  // Walked, and it did exactly this: a back office one minute old read a file,
  // matched it against an empty copy of the catalogue, called every row new,
  // and left the shop with two of everything under one code.
  assert.match(tooEarlyToMatch({ everSynced: false, moreToPull: false }), /not read the shop/);
  assert.match(tooEarlyToMatch({ everSynced: true, moreToPull: true }), /still reading/);
  assert.equal(tooEarlyToMatch({ everSynced: true, moreToPull: false }), null);
});

test('a shop’s own spreadsheet reads, headings and all', () => {
  const read = readCatalogue(
    [
      'Item Name,Price (taka),Code,Barcode,VAT %,Unit,Cost,Category',
      'Rice Miniket 5kg,430,RICE5,8690000000001,15,kg,380,Rice',
      '"Soap, the small one",35,SOAP1,,15,Nos,28,Soap',
    ].join('\n'),
  );

  assert.equal(read.fault, null);
  assert.equal(read.rows.length, 2);
  assert.equal(read.rows[0].name, 'Rice Miniket 5kg');
  assert.equal(read.rows[0].price_minor, 43_000);
  assert.equal(read.rows[0].cost_minor, 38_000);
  assert.equal(read.rows[0].vat_bp, 1_500);
  assert.equal(read.rows[0].category, 'Rice');
  // A comma inside a name is what quotes are for, and a shop's own file has
  // them: "Soap, the small one" is one column, not two.
  assert.equal(read.rows[1].name, 'Soap, the small one');
  assert.equal(read.rows[1].barcode, '');
});

test('a file whose first row is not headings is refused, with what to do', () => {
  const read = readCatalogue('Rice Miniket 5kg,430\nSoap,35');
  assert.equal(read.rows.length, 0);
  assert.match(read.fault, /name.*price/i);
});

test('a price a shop writes its own way still reads', () => {
  const read = readCatalogue(['name,price', 'Rice,"1,250.50"', 'Oil,৳185'].join('\n'));
  assert.equal(read.rows[0].price_minor, 125_050);
  assert.equal(read.rows[1].price_minor, 18_500);
});

test('a row nobody can read comes back saying so, rather than vanishing', () => {
  // A shop that imports eight hundred lines and gets seven hundred and ninety
  // has lost ten things it will find at the counter.
  const read = readCatalogue(
    ['name,price,vat', 'Rice,430,15', ',430,15', 'Oil,about two hundred,15', 'Dal,140,lots'].join(
      '\n',
    ),
  );
  const { ready, refused } = whatWillBeWritten(read.rows);
  assert.equal(ready.length, 1);
  assert.equal(refused.length, 3);
  assert.deepEqual(refused[0].wrong, ['no name']);
  assert.deepEqual(refused[1].wrong, ['no price anybody can read']);
  assert.deepEqual(refused[2].wrong, ['a VAT rate nobody can read']);
  assert.equal(refused[0].line, 3, 'the line in their file, so they can find it');
});

test('an empty VAT column is left to the shop rather than guessed at', () => {
  const read = readCatalogue(['name,price,vat', 'Rice,430,'].join('\n'));
  assert.equal(read.rows[0].vat_bp, null);
});

test('importing the same file twice corrects rather than duplicates', () => {
  const read = readCatalogue(
    ['name,price,code,barcode', 'Rice Miniket 5kg,450,RICE5,8690000000001', 'Tea,220,TEA1,'].join(
      '\n',
    ),
  );
  const known = [
    { id: 'held-rice', code: 'RICE5', barcodes: ['8690000000001'], name: 'Rice Miniket 5kg' },
  ];

  const matched = against(read.rows, known);
  assert.equal(matched[0].matched?.id, 'held-rice', 'the shop already sells this one');
  assert.equal(matched[1].matched, null, 'and has never heard of this one');
});

test('a code the shop wrote in another case is still the same item', () => {
  const read = readCatalogue(['name,price,code', 'Rice,450,rice5'].join('\n'));
  const matched = against(read.rows, [{ id: 'held', code: 'RICE5', barcodes: [] }]);
  assert.equal(matched[0].matched?.id, 'held');
});

test('nothing in the file is nothing to do', () => {
  assert.equal(readCatalogue('').rows.length, 0);
  assert.match(readCatalogue('').fault, /nothing in it/);
});
