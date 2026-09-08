/// Reading a shop's catalogue out of the spreadsheet it already keeps.
///
/// A shop with eight hundred lines will not type them into a form, and every
/// one of them is already in a file somebody made: a price list from the
/// wholesaler, a stock sheet, an export from whatever they used before. Until
/// this, the answer to "how do I get my items in" was "one at a time", which is
/// the answer that ends the conversation.
///
/// Nothing here talks to a shop. It reads text and says what it found, so the
/// screen can put the whole thing in front of somebody before a single row is
/// written: an import nobody previewed is how a shop ends up with two of
/// everything at the wrong price.

/// Which character separates the columns.
///
/// Excel writes semicolons wherever the machine's decimal separator is a comma,
/// which is most of Europe and any laptop somebody set up that way, and tabs
/// come out of anything pasted from a sheet. A file the shop already has is the
/// file it has: refusing it because of a punctuation mark is refusing the shop.
///
/// Decided from the heading row by counting, because that row is the one line
/// guaranteed to hold every separator once.
function separatorOf(heading) {
  const counts = [
    [',', (heading.match(/,/g) ?? []).length],
    [';', (heading.match(/;/g) ?? []).length],
    ['\t', (heading.match(/\t/g) ?? []).length],
  ];
  counts.sort((one, other) => other[1] - one[1]);
  return counts[0][1] === 0 ? ',' : counts[0][0];
}

/// Split one line of a separated file, honouring quotes.
///
/// Written out rather than pulled in, because the whole of what a shop's
/// spreadsheet needs is: separators inside quotes, and doubled quotes meaning
/// one. A dependency here is a dependency in the thing a shop runs.
function fields(line, separator = ',') {
  const out = [];
  let held = '';
  let quoted = false;
  for (let at = 0; at < line.length; at += 1) {
    const ch = line[at];
    if (quoted) {
      if (ch === '"') {
        if (line[at + 1] === '"') {
          held += '"';
          at += 1;
        } else {
          quoted = false;
        }
      } else {
        held += ch;
      }
      continue;
    }
    if (ch === '"') {
      quoted = true;
    } else if (ch === separator) {
      out.push(held);
      held = '';
    } else {
      held += ch;
    }
  }
  out.push(held);
  return out.map((one) => one.trim());
}

/// What each column means, by the words a shop's own file is likely to use.
///
/// Matched loosely on purpose: a file that says "Item Name" or "নাম" or "price
/// (taka)" is the file the shop has, and refusing it because the heading is not
/// the expected word is refusing the shop.
const COLUMNS = [
  ['name', ['name', 'item', 'item name', 'product', 'description', 'নাম']],
  ['price', ['price', 'rate', 'mrp', 'selling price', 'price (taka)', 'দাম']],
  ['code', ['code', 'sku', 'item code', 'product code']],
  ['barcode', ['barcode', 'bar code', 'ean', 'upc']],
  ['vat', ['vat', 'vat %', 'tax', 'tax %', 'vat rate']],
  ['unit', ['unit', 'sold by', 'uom']],
  ['cost', ['cost', 'buy price', 'purchase price', 'cost price']],
  ['category', ['category', 'kind', 'group', 'type']],
  ['name_bn', ['bangla', 'bangla name', 'name (bangla)', 'bn']],
];

/// Which column holds what, from the heading row.
function headings(row) {
  const found = {};
  row.forEach((heading, at) => {
    const said = heading.trim().toLowerCase();
    for (const [field, names] of COLUMNS) {
      if (found[field] === undefined && names.includes(said)) found[field] = at;
    }
  });
  return found;
}

/// A number as a shop writes one: "430", "430.00", "1,250", "৳430".
///
/// Returns null for anything else, which the row then reports rather than
/// guessing at: a price read wrongly is a shelf priced wrongly.
function amount(text) {
  if (text === undefined || text === null) return null;
  const cleaned = String(text).replace(/[,\s৳]/g, '');
  if (cleaned === '') return null;
  const value = Number(cleaned);
  return Number.isFinite(value) ? value : null;
}

/// The most a shop charges for one of anything, in taka.
///
/// Ten crore. Past this it is not a price, it is another column read as one: a
/// phone number, a barcode, a date a spreadsheet turned into a serial. It is
/// also where exactness in a browser starts to matter, which is a worse way to
/// find out.
const TOO_MUCH = 100_000_000;

/// Read a catalogue out of comma-separated text.
///
/// Every row comes back, good or bad, with what is wrong said in words. A row
/// nobody can price is not silently dropped: a shop that imports eight hundred
/// lines and gets seven hundred and ninety has lost ten things it will find at
/// the counter.
export function readCatalogue(text) {
  const lines = String(text ?? '')
    // A byte order mark, which is what Excel puts at the front of every CSV it
    // saves as UTF-8. Left in, it makes the first heading "\ufeffname", nothing
    // matches, and the shop is told its own export is not a catalogue.
    .replace(/^\ufeff/, '')
    .split(/\r?\n/)
    .filter((line) => line.trim() !== '');
  if (lines.length === 0) return { columns: {}, rows: [], fault: 'that file has nothing in it' };

  const separator = separatorOf(lines[0]);
  const columns = headings(fields(lines[0], separator));
  if (columns.name === undefined || columns.price === undefined) {
    return {
      columns,
      rows: [],
      fault:
        'the first row has to name the columns, and it needs at least a name and a price: ' +
        'try "name,price,code,barcode,vat,unit,cost,category"',
    };
  }

  const rows = [];
  for (let at = 1; at < lines.length; at += 1) {
    const cells = fields(lines[at], separator);
    const said = (field) => (columns[field] === undefined ? '' : (cells[columns[field]] ?? ''));
    const name = said('name');
    const price = amount(said('price'));
    const vat = amount(said('vat'));
    const cost = amount(said('cost'));

    const wrong = [];
    if (name === '') wrong.push('no name');
    if (price === null) wrong.push('no price anybody can read');
    else if (price < 0) wrong.push('a price below nothing');
    // A shop does not sell anything for ten crore taka, and a number that large
    // is a column read as a price: a phone number, a barcode, a date somebody's
    // spreadsheet turned into a serial. Beyond this the arithmetic stops being
    // exact in a browser at all, which is a worse way to find out.
    else if (price > TOO_MUCH) wrong.push('a price too large to be one');
    if (said('vat') !== '' && vat === null) wrong.push('a VAT rate nobody can read');
    else if (vat !== null && (vat < 0 || vat > 100)) wrong.push('a VAT rate that is not a rate');
    if (said('cost') !== '' && cost === null) wrong.push('a cost nobody can read');
    else if (cost !== null && cost < 0) wrong.push('a cost below nothing');
    else if (cost !== null && cost > TOO_MUCH) wrong.push('a cost too large to be one');

    rows.push({
      line: at + 1,
      name,
      name_bn: said('name_bn'),
      code: said('code'),
      barcode: said('barcode'),
      unit: said('unit'),
      category: said('category'),
      // Poisha, like every amount that crosses into the shop.
      price_minor: price === null ? null : Math.round(price * 100),
      cost_minor: cost === null ? 0 : Math.round(cost * 100),
      // Basis points. An empty column means the shop's ordinary rate, which the
      // screen supplies: this file cannot know what that is.
      vat_bp: vat === null ? null : Math.round(vat * 100),
      wrong,
    });
  }
  return { columns, rows: sameTwice(rows), fault: null };
}

/// Mark rows that repeat a code or a barcode already used further up the file.
///
/// One code belongs to one item: a file saying otherwise would create two, and
/// which of them a scan rings is whichever the index happened to keep. The
/// first row keeps the code and the later ones are refused by line number, so
/// somebody can look at their own file and see which pair to fix.
function sameTwice(rows) {
  const codeAt = new Map();
  const barcodeAt = new Map();
  return rows.map((row) => {
    const code = row.code.trim().toLowerCase();
    const barcode = row.barcode.trim();
    const wrong = [...row.wrong];
    if (code) {
      const first = codeAt.get(code);
      if (first === undefined) codeAt.set(code, row.line);
      else wrong.push(`the same code as line ${first}`);
    }
    if (barcode) {
      const first = barcodeAt.get(barcode);
      if (first === undefined) barcodeAt.set(barcode, row.line);
      else wrong.push(`the same barcode as line ${first}`);
    }
    return { ...row, wrong };
  });
}

/// The rows worth writing, and the ones to show somebody first.
export function whatWillBeWritten(rows) {
  return {
    ready: (rows ?? []).filter((row) => row.wrong.length === 0),
    refused: (rows ?? []).filter((row) => row.wrong.length > 0),
  };
}

/// Whether this device knows enough about the shop to match a file against it.
///
/// Found by walking it. A back office that had just enrolled read a file and
/// matched it against a copy of the catalogue that was still empty, so every
/// row looked new and the shop ended up with two of each: two "Rice Miniket
/// 5kg", both code RICE5, one of them with the stock and the other with the
/// sales. The screen said "new" for all of them and meant "I have not looked".
///
/// So the file is not read at all until this device has pulled the catalogue to
/// the end. A wrong answer here is not a slow import, it is a shop with a
/// duplicate of everything it sells.
export function tooEarlyToMatch({ everSynced, moreToPull }) {
  if (!everSynced) {
    return (
      'this device has not read the shop yet. Wait for the line at the top to say it has ' +
      'reached the shop, then choose the file again: matching against a catalogue this device ' +
      'has not read would add everything a second time.'
    );
  }
  if (moreToPull) {
    return (
      'this device is still reading the shop’s catalogue. Wait for it to finish, then choose ' +
      'the file again: anything it has not read yet would be added a second time.'
    );
  }
  return null;
}

/// Match what was read against what the shop already sells, by code and then by
/// barcode.
///
/// Importing the same file twice is the ordinary mistake, and a shop that ends
/// up with two of everything at two prices has a worse problem than the one it
/// was solving. Anything that matches is a correction to what is already there;
/// only the rest are new.
export function against(rows, known) {
  const byCode = new Map();
  const byBarcode = new Map();
  for (const item of known ?? []) {
    if (item.code) byCode.set(String(item.code).trim().toLowerCase(), item);
    for (const barcode of item.barcodes ?? []) byBarcode.set(String(barcode).trim(), item);
  }
  return (rows ?? []).map((row) => {
    const matched =
      (row.code && byCode.get(row.code.toLowerCase())) ||
      (row.barcode && byBarcode.get(row.barcode)) ||
      null;
    return { ...row, matched };
  });
}
