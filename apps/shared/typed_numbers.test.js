import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { minorFrom } from './money.js';
import { milliFrom } from './quantity.js';

/// Every screen file in both apps.
function screens() {
  const here = dirname(fileURLToPath(import.meta.url));
  const roots = [join(here, '..', 'admin', 'src'), join(here, '..', 'till-web', 'src')];
  const found = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.name.endsWith('.svelte')) found.push(path);
    }
  };
  for (const root of roots) walk(root);
  return found;
}

/// The places a screen reads a number with `Number()` and may keep doing so,
/// each with the reason it is not a figure somebody typed into a box.
///
/// Written out rather than inferred, because the difference cannot be seen in
/// the text: `Number(x)` reads the same whether x came from a dropdown, from
/// the core, or from a cashier's fingers. A new one has to be argued for here,
/// and arguing for it is the point.
const NOT_TYPED_BY_A_PERSON = new Map([
  ['Number(movePercent)', 'a bulk reprice, and nothing is written until the owner reads the list of what each price would become'],
  ['Number(shopStockRule)', 'a dropdown with three settings in it'],
  ['Number(daysWanted)', 'how far back a low-stock list looks, which shows a list and writes nothing'],
  ['Number(bringingInVat)', 'a fallback tax rate, refused outside nought to a hundred before anything is written'],
  ['Number(itemSupply)', 'a dropdown: standard, zero rated, exempt'],
  ['Number(parts.how_far)', 'a figure the core put in a refusal, to pick between "1 hour" and "2 hours"'],
]);

test('a figure somebody typed is never read by Number()', () => {
  // What this is against, found by walking the till: "1e3" in the box marked
  // "Cash taken" registered a thousand taka against a two hundred and fifty
  // three taka sale, and the till offered seven hundred and forty seven in
  // change. `Number("1e3")` is a thousand. So is `Number("0x3e8")`.
  //
  // The same three characters in "How many gone" wrote a thousand units off a
  // shelf, on the one stock screen with no list to read before it writes.
  //
  // The parsers refuse both, and refuse "-3" rather than quietly taking its
  // size, which is what the delivery screen was already doing and what the rest
  // of these now do.
  const offenders = [];
  for (const path of screens()) {
    const source = readFileSync(path, 'utf8');
    for (const [index, line] of source.split('\n').entries()) {
      const trimmed = line.trim();
      if (trimmed.startsWith('//') || trimmed.startsWith('*')) continue;
      for (const call of line.matchAll(/Number\([^)]*\)/g)) {
        const text = call[0];
        if (text.startsWith('Number.is')) continue;
        if (NOT_TYPED_BY_A_PERSON.has(text)) continue;
        offenders.push(`${path.split('/apps/')[1]}:${index + 1}  ${trimmed.slice(0, 90)}`);
      }
    }
  }
  assert.deepEqual(
    offenders,
    [],
    `read it with minorFrom or milliFrom, or say here why it is not a figure somebody typed:\n${offenders.join('\n')}`,
  );
});

test('the parsers refuse what a person cannot type', () => {
  // The property the screens are now leaning on, stated where they lean on it.
  for (const typed of ['1e3', '0x10', 'Infinity', '-3', '1 000', '', ' ', 'abc', '1.2.3']) {
    assert.equal(minorFrom(typed), null, `money took "${typed}"`);
    assert.equal(milliFrom(typed), null, `a quantity took "${typed}"`);
  }
  // And take what a person does type.
  assert.equal(minorFrom('253'), 25_300);
  assert.equal(minorFrom('253.75'), 25_375);
  assert.equal(milliFrom('1.5'), 1_500);
  assert.equal(milliFrom('25'), 25_000);
});
