import { test } from 'node:test';
import assert from 'node:assert/strict';

import { everyScreen } from './screens.js';

/// A panel that uses something it was never given.
///
/// A screen split into panels has one failure nothing else here catches. A
/// panel's markup reads `money(...)` or `tills.find(...)`; the value used to be
/// a variable in the same file and is now a prop, and if the screen forgets to
/// pass it the build is clean, every test is green, and the panel throws where
/// it stands. Svelte renders nothing for the block that threw, so what a
/// shopkeeper sees is a panel that silently shows less than it did.
///
/// Met four times in one afternoon of splitting: the import preview printed
/// money it had not been handed, the trail named a till from a list it did not
/// have, the who-owes list called its loader by the name it had before the
/// move, and an item a till wrote down opened a form that had stayed behind on
/// the screen. All four built cleanly.
///
/// This reads what each file's markup uses and what its script has, and says so
/// when the first is not covered by the second.

/// What a Svelte block header introduces: loop variables, `{@const}` names, and
/// the key beside them. These are declared by the markup itself.
function boundByBlocks(markup) {
  const names = new Set();
  for (const [, one] of markup.matchAll(/\{#each\s+[^}]*?\s+as\s+([A-Za-z_$][\w$]*)/g)) {
    names.add(one);
  }
  // `{#each rows as row, at}` and `{#each rows as row (row.id)}`.
  for (const [, one] of markup.matchAll(/\{#each\s+[^}]*?\s+as\s+[A-Za-z_$][\w$]*\s*,\s*([A-Za-z_$][\w$]*)/g)) {
    names.add(one);
  }
  for (const [, one] of markup.matchAll(/\{@const\s+([A-Za-z_$][\w$]*)/g)) names.add(one);
  for (const [, one] of markup.matchAll(/\{#await[^}]*?\s+then\s+([A-Za-z_$][\w$]*)/g)) names.add(one);
  // Arrow parameters written inside the markup: `(one) => one.id`.
  for (const [, one] of markup.matchAll(/\(?\b([A-Za-z_$][\w$]*)\)?\s*=>/g)) names.add(one);
  for (const [, group] of markup.matchAll(/\(([^)]*)\)\s*=>/g)) {
    for (const part of group.split(',')) {
      const name = part.trim().split(/[:=\s]/)[0];
      if (/^[A-Za-z_$][\w$]*$/.test(name)) names.add(name);
    }
  }
  return names;
}

/// What the script half declares: state, constants, functions, props, imports.
///
/// Comments come out first. The prop list carries doc comments between its
/// names, and a scan that read those would take `///` for a name and miss the
/// prop underneath it, which is a guard that fails on the files it is for.
function declaredIn(source) {
  const script = source.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/\/\/[^\n]*/g, ' ');
  const names = new Set();
  for (const [, one] of script.matchAll(/(?:^|\s)(?:let|const|var)\s+([A-Za-z_$][\w$]*)/g)) names.add(one);
  for (const [, one] of script.matchAll(/function\s+([A-Za-z_$][\w$]*)/g)) names.add(one);
  for (const [, group] of script.matchAll(/let\s*\{([\s\S]*?)\}\s*=\s*\$props\(\)/g)) {
    for (const part of group.split(',')) {
      const name = part.trim().split(/[:=\s]/)[0].replace(/^\.\.\./, '');
      if (/^[A-Za-z_$][\w$]*$/.test(name)) names.add(name);
    }
  }
  for (const [, group] of script.matchAll(/import\s*\{([^}]*)\}/g)) {
    for (const part of group.split(',')) {
      const name = part.trim().split(/\s+as\s+/).pop().trim();
      if (/^[A-Za-z_$][\w$]*$/.test(name)) names.add(name);
    }
  }
  for (const [, one] of script.matchAll(/import\s+([A-Za-z_$][\w$]*)\s+from/g)) names.add(one);
  return names;
}

/// The globals a screen may use without anybody handing them over.
const ALWAYS_THERE = new Set([
  'String', 'Number', 'Boolean', 'Math', 'Date', 'JSON', 'Object', 'Array',
  'console', 'window', 'document', 'navigator', 'localStorage', 'crypto',
  'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'fetch',
  'Promise', 'Set', 'Map', 'URL', 'Blob', 'File', 'FileReader', 'Intl',
  'parseInt', 'parseFloat', 'isNaN', 'undefined', 'null', 'true', 'false',
  'if', 'else', 'each', 'await', 'then', 'catch', 'const', 'key', 'this', 'async',
]);

test('no panel uses something nobody handed it', () => {
  for (const { path, source } of everyScreen()) {
    const at = source.indexOf('</script>');
    if (at < 0) continue;
    const script = source.slice(0, at);
    // The markup, and not the stylesheet under it: a `<style>` block is full of
    // things that read like calls, `calc(...)` and `rgba(...)` among them, and
    // none of them is a name anybody has to hand over.
    const rest = source.slice(at);
    const style = rest.indexOf('<style>');
    const markup = style < 0 ? rest : rest.slice(0, style);
    const known = declaredIn(script);
    const bound = boundByBlocks(markup);

    // Called as a function, or read as an object: `money(x)`, `tills.find(...)`,
    // `{names[id]}`. These are the two shapes that throw rather than rendering
    // an empty string, which is what makes them worth a test.
    const used = new Set();
    for (const [, one] of markup.matchAll(/[{(\s,]([a-z][\w$]*)\s*\(/g)) used.add(one);
    for (const [, one] of markup.matchAll(/\{\s*([a-z][\w$]*)\s*[.[]/g)) used.add(one);

    const missing = [...used].filter(
      (one) => !known.has(one) && !bound.has(one) && !ALWAYS_THERE.has(one),
    );
    assert.deepEqual(
      missing,
      [],
      `${path} uses ${missing.join(', ')} in its markup and nothing in its script declares ` +
        `it. A panel is handed what it needs as props: this builds cleanly, passes every other ` +
        `test, and throws where it stands, and Svelte renders nothing for the block that threw.`,
    );
  }
});
