import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import {
  against,
  movedALot,
  notReadBackYet,
  readCatalogue,
  tooEarlyToMatch,
  whatWillBeWritten,
  writeCatalogue,
} from './catalogue_file.js';

test('a file is not read again until this device can see what it just wrote', () => {
  // Walked. Two rows were written, the same file was read a moment later, and
  // both read as new again: the rows were on the shop's server and not yet in
  // this device's copy, which is what the matching is done against. Writing
  // again would have left the shop with two of each.
  //
  // The first attempt asked "have I finished syncing" and lost the race: a
  // pull already in flight when the write landed answered yes, because it did
  // have everything it had asked for, and it had asked before the rows
  // existed. So the question is the one that matters directly.
  const wrote = [{ code: 'RICE5' }, { code: 'DAL1' }];
  assert.equal(notReadBackYet(wrote, []), 2);
  assert.equal(notReadBackYet(wrote, [{ code: 'RICE5' }]), 1, 'a pull that brought half back');
  assert.equal(notReadBackYet(wrote, [{ code: 'rice5' }, { code: 'DAL1' }, { code: 'X' }]), 0);

  // By code and barcode, not by id. Matching on the id was walked and left the
  // back office refusing every import from then on: the id is minted here as a
  // string, travels as a number, and comes back written the shop's way, so the
  // one that went out never equals the one that came back. These are the keys
  // `against` matches on, which is the question the next import actually asks.
  assert.equal(notReadBackYet([{ barcode: '8901' }], [{ barcodes: ['8901'] }]), 0);
  assert.equal(notReadBackYet([{ barcode: '8901' }], [{ barcodes: ['8902'] }]), 1);

  // Nothing written is nothing to wait for, which is every import but the
  // second one in a row.
  assert.equal(notReadBackYet([], [{ code: 'RICE5' }]), 0);
  assert.equal(notReadBackYet(undefined, undefined), 0);
});

test('a device that has not read the shop may not match a file against it', () => {
  // Walked, and it did exactly this: a back office one minute old read a file,
  // matched it against an empty copy of the catalogue, called every row new,
  // and left the shop with two of everything under one code.
  assert.match(tooEarlyToMatch({ everSynced: false, moreToPull: false }), /not read the shop/);
  assert.match(tooEarlyToMatch({ everSynced: true, moreToPull: true }), /still reading/);
  assert.equal(tooEarlyToMatch({ everSynced: true, moreToPull: false }), null);

  // Taking the list out fails differently, and a message naming the wrong
  // consequence is one somebody argues with instead of waiting.
  assert.match(
    tooEarlyToMatch({ everSynced: true, moreToPull: true }, 'taking the list out'),
    /missing whatever it has not read/,
  );
  assert.match(
    tooEarlyToMatch({ everSynced: true, moreToPull: true }),
    /added a second time/,
  );

  // And a device that cannot reach the shop at all is not "still reading": it
  // is stopped, and telling somebody to wait for it to finish is telling them
  // to wait for something that is not happening.
  assert.match(
    tooEarlyToMatch({ everSynced: true, moreToPull: false, reaching: false }),
    /cannot reach the shop/,
  );
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
  assert.deepEqual(refused[0].wrong, [{ code: 'no-name' }]);
  assert.deepEqual(refused[1].wrong, [{ code: 'no-price' }]);
  assert.deepEqual(refused[2].wrong, [{ code: 'vat-unreadable' }]);
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

test('a file Excel saved is still a file', () => {
  // A byte order mark at the front, which is what Excel writes when it saves as
  // UTF-8. Left in, the first heading is "﻿name", nothing matches, and the
  // shop is told its own export is not a catalogue.
  const read = readCatalogue('﻿name,price\r\nRice,430\r\n');
  assert.equal(read.fault, null);
  assert.equal(read.rows[0].name, 'Rice');
  assert.equal(read.rows[0].price_minor, 43_000);
});

test('a sheet that separates with semicolons or tabs reads the same', () => {
  // Excel writes semicolons wherever the decimal separator is a comma, and tabs
  // come out of anything pasted from a sheet.
  const semi = readCatalogue(['name;price;code', 'Rice;430;RICE5'].join('\n'));
  assert.equal(semi.fault, null);
  assert.equal(semi.rows[0].price_minor, 43_000);
  assert.equal(semi.rows[0].code, 'RICE5');

  const tabs = readCatalogue(['name\tprice\tcode', 'Rice\t430\tRICE5'].join('\n'));
  assert.equal(tabs.rows[0].code, 'RICE5');
});

test('one code belongs to one item, even inside one file', () => {
  // Two rows under one code would create two items, and which one a scan rings
  // is whichever the index happened to keep: the wrong price and the wrong
  // thing off the shelf, with nothing on any screen to say why.
  const read = readCatalogue(
    [
      'name,price,code,barcode',
      'Rice Miniket 5kg,430,RICE5,8690000000001',
      'Rice Miniket sack,450,RICE5,',
      'Something else,50,ELSE,8690000000001',
    ].join('\n'),
  );
  const { ready, refused } = whatWillBeWritten(read.rows);
  assert.equal(ready.length, 1);
  assert.deepEqual(refused[0].wrong, [{ code: 'same-code-as', fill: { line: 2 } }]);
  assert.deepEqual(refused[1].wrong, [{ code: 'same-barcode-as', fill: { line: 2 } }]);
});

test('a number too large to be a price is another column read as one', () => {
  // A phone number, a barcode, a date a spreadsheet turned into a serial. Ten
  // crore is past anything a shop charges for one of something.
  const read = readCatalogue(
    ['name,price,cost,vat', 'Rice,01711234567,,15', 'Oil,185,-20,15', 'Dal,140,,900'].join('\n'),
  );
  const { ready, refused } = whatWillBeWritten(read.rows);
  assert.equal(ready.length, 0);
  assert.deepEqual(refused[0].wrong, [{ code: 'price-too-large' }]);
  assert.deepEqual(refused[1].wrong, [{ code: 'cost-below-nothing' }]);
  assert.deepEqual(refused[2].wrong, [{ code: 'vat-not-a-rate' }]);
});

test('a list taken out comes back in unchanged', () => {
  // The round trip is what makes the import safe to use on a price rise: take
  // the shop's own list out, edit one column in the spreadsheet they already
  // know, bring it back. Not a claim: the same two functions, run into each
  // other.
  const held = [
    {
      id: 'a',
      name: 'Rice Miniket 5kg',
      name_bn: 'মিনিকেট চাল ৫ কেজি',
      code: 'RICE5',
      barcodes: ['8690000000001'],
      price_minor: 43_000,
      vat_bp: 1_500,
      unit: 'kg',
      cost_minor: 38_000,
      category: 'Rice',
    },
    {
      id: 'b',
      name: 'Soap, the small one',
      name_bn: 'Soap, the small one',
      code: 'SOAP1',
      barcodes: [],
      price_minor: 3_500,
      vat_bp: 0,
      unit: 'Nos',
      cost_minor: 0,
      category: '',
    },
  ];

  const read = readCatalogue(writeCatalogue(held));
  assert.equal(read.fault, null);
  assert.equal(read.rows.length, 2);
  assert.deepEqual(
    read.rows.map((row) => [row.name, row.code, row.barcode, row.price_minor, row.vat_bp, row.unit, row.cost_minor, row.category]),
    [
      ['Rice Miniket 5kg', 'RICE5', '8690000000001', 43_000, 1_500, 'kg', 38_000, 'Rice'],
      ['Soap, the small one', 'SOAP1', '', 3_500, 0, 'Nos', 0, ''],
    ],
  );
  assert.equal(read.rows[0].name_bn, 'মিনিকেট চাল ৫ কেজি');
  // A Bangla name nobody typed is a copy of the English one, and comes back
  // blank rather than as the same words twice.
  assert.equal(read.rows[1].name_bn, '');

  // And every row matches what it came from, so bringing it back corrects
  // rather than adding the shop a second time.
  const matched = against(read.rows, held);
  assert.deepEqual(matched.map((row) => row.matched?.id), ['a', 'b']);
});

test('a price a long way from the one the shop holds is put in front of somebody', () => {
  // An import can reprice eight hundred lines in one press and the preview
  // shows twenty. A formula dragged one row too far looks like an ordinary row
  // on the screen and like a shelf nobody can explain in the morning.
  const read = readCatalogue(
    [
      'name,price,code',
      'Rice Miniket 5kg,4300,RICE5',
      'Soybean Oil 1L,190,OIL1',
      'Tea 400g,110,TEA4',
      'Something new,900,NEW1',
    ].join('\n'),
  );
  const known = [
    { id: 'rice', code: 'RICE5', barcodes: [], price_minor: 43_000 },
    { id: 'oil', code: 'OIL1', barcodes: [], price_minor: 18_500 },
    { id: 'tea', code: 'TEA4', barcodes: [], price_minor: 22_000 },
  ];

  const looked = movedALot(against(read.rows, known));
  assert.deepEqual(
    looked.map((row) => [row.code, row.was_minor, row.price_minor]),
    [
      ['RICE5', 43_000, 430_000],
      ['TEA4', 22_000, 11_000],
    ],
    'ten times up and half down, and not the one that moved by taka',
  );
});

test('a shop can say which of its goods are exempt', () => {
  // Zero rated and exempt both charge nothing and are declared in different
  // places. Without a column for it, everything a shop brought in landed
  // standard rated and its return declared tax on goods that carry none.
  const read = readCatalogue(
    [
      'name,price,supply',
      'Rice,430,exempt',
      'Book,220,zero rated',
      'Soap,35,standard',
      'Oil,185,',
      'Dal,140,whatever',
    ].join('\n'),
  );
  const { ready, refused } = whatWillBeWritten(read.rows);
  assert.deepEqual(
    ready.map((row) => [row.name, row.supply]),
    [
      ['Rice', 2],
      ['Book', 1],
      ['Soap', 0],
      // Nothing said: an item the shop already sells keeps what it was, and a
      // new one is standard, which the screen decides rather than this.
      ['Oil', null],
    ],
  );
  assert.deepEqual(refused[0].wrong, [{ code: 'supply-unreadable' }]);
});

test('what a shop is exempt from survives the round trip', () => {
  const held = [
    { id: 'a', name: 'Rice', code: 'RICE', barcodes: [], price_minor: 43_000, vat_bp: 0, unit: 'kg', cost_minor: 0, category: '', supply: 2 },
    { id: 'b', name: 'Soap', code: 'SOAP', barcodes: [], price_minor: 3_500, vat_bp: 1_500, unit: 'Nos', cost_minor: 0, category: '', supply: 0 },
  ];
  const read = readCatalogue(writeCatalogue(held));
  assert.deepEqual(
    read.rows.map((row) => [row.name, row.supply]),
    [
      ['Rice', 2],
      ['Soap', 0],
    ],
  );
});

test('a shelf priced at MRP is read as one', () => {
  // A price that already carries the tax is what an MRP is, and a great many
  // shelves here are priced that way. Read as tax exclusive, the till adds
  // fifteen percent on top of a price that already had it: every shelf wrong.
  const read = readCatalogue(
    [
      'name,price,price includes vat',
      'Biscuits,20,yes',
      'Rice,430,no',
      'Oil,185,',
      'Soap,35,perhaps',
    ].join('\n'),
  );
  const { ready, refused } = whatWillBeWritten(read.rows);
  assert.deepEqual(
    ready.map((row) => [row.name, row.price_inclusive]),
    [
      ['Biscuits', true],
      ['Rice', false],
      ['Oil', null],
    ],
  );
  assert.deepEqual(refused[0].wrong, [{ code: 'inclusive-unreadable' }]);

  // And in the shop's own language, because the column is theirs to fill in.
  const bangla = readCatalogue(['name,price,mrp', 'Biscuits,20,হ্যাঁ'].join('\n'));
  assert.equal(bangla.rows[0].price_inclusive, true);
});

test('a price rule survives the round trip', () => {
  const held = [
    { id: 'a', name: 'Biscuits', code: 'BIS', barcodes: [], price_minor: 2_000, vat_bp: 1_500, unit: 'Nos', cost_minor: 0, category: '', supply: 0, price_inclusive: true },
    { id: 'b', name: 'Rice', code: 'RICE', barcodes: [], price_minor: 43_000, vat_bp: 1_500, unit: 'kg', cost_minor: 0, category: '', supply: 0, price_inclusive: false },
  ];
  const read = readCatalogue(writeCatalogue(held));
  assert.deepEqual(
    read.rows.map((row) => [row.name, row.price_inclusive]),
    [
      ['Biscuits', true],
      ['Rice', false],
    ],
  );
});
