/// Every command a till can be given is run by a screen, or is written down
/// here as reached by something that is not one.
///
/// The third link in a chain the server holds the first of. A command is not a
/// request to the shop: it is what a screen asks its own till to do, and the
/// same dead-code question applies. A command with tests and no caller is green,
/// covered, and does nothing for a cashier.
///
/// Nine of these are reached by something other than a screen. That is a fine
/// answer and a bad thing to have to discover by grepping in a year, so each one
/// says who reaches it. The list is the point of this file: it is short, it has
/// to be justified line by line, and it fails when it goes stale in either
/// direction.
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const COMMANDS = JSON.parse(
  readFileSync(new URL('./till_commands.json', import.meta.url), 'utf8'),
);

/// Everywhere a screen runs a command.
const CALLERS = [
  '../admin/src/App.svelte',
  '../till-web/src/App.svelte',
  './counting.js',
  './buying.js',
  './records.js',
  './people.js',
  './repricing.js',
  './commands.js',
  './till.js',
];

/// Commands no screen runs with `op:`, and who reaches them instead.
///
/// Two kinds, and the difference matters. Most are reached through a named
/// method on the bridge, because they carry something a plain command object
/// cannot: a credential, a worker's own loop, or a whole catalogue. The rest are
/// reached only across the C ABI or by tests, which is a real caller for a
/// platform that is not built yet and worth saying out loud rather than leaving
/// as nine names nothing greps for.
const REACHED_BY_SOMETHING_ELSE = {
  admin: 'the bridge’s admin(), because the request carries the device’s credential',
  enrol: 'the bridge’s enrol(), which trades a code for that credential',
  view: 'till.view(), called by the worker after every command',
  sync_step: 'the sync loop in till.worker.js, which is where it belongs: a hidden tab’s timers stop',
  sync_apply: 'the same loop, with what the shop answered',
  sync_failed: 'the same loop, when the shop could not be reached',
  apply_items:
    'the C ABI, for a native till handing its own catalogue in. No Android UI exists yet, so today its only callers are tests in bindings and ffi',
  escpos:
    'nothing yet. The thermal path is built and no code writes its bytes to a printer, which is written down in todo.md rather than hidden here',
  paper_bytes:
    'nothing yet, for the same reason: it is the other half of the thermal path',
};

function run() {
  const found = new Set();
  for (const caller of CALLERS) {
    const source = readFileSync(new URL(caller, import.meta.url), 'utf8');
    for (const [, op] of source.matchAll(/op:\s*'([a-z_]+)'/g)) found.add(op);
  }
  return found;
}

test('every command a till can be given is run by a screen or accounted for', () => {
  const asked = run();
  for (const command of COMMANDS) {
    if (command in REACHED_BY_SOMETHING_ELSE) continue;
    assert.ok(
      asked.has(command),
      `nothing runs '${command}'. Either a screen is missing, which is the defect this test ` +
        `exists for, or it belongs in REACHED_BY_SOMETHING_ELSE with the reason written down.`,
    );
  }
});

test('every command a screen runs is one the till knows', () => {
  const known = new Set(COMMANDS);
  for (const op of run()) {
    assert.ok(
      known.has(op),
      `a screen runs '${op}' and the till has no such command: a typo here is invisible until ` +
        `somebody presses that button`,
    );
  }
});

test('nothing is excused that no longer exists, or that a screen now runs', () => {
  // Both directions, or the list keeps reasons for commands that were deleted
  // and excuses for ones a screen picked up in the meantime. An excuse nobody
  // needs is worse than no list: it is a reason somebody will trust.
  const known = new Set(COMMANDS);
  const asked = run();
  for (const [command, why] of Object.entries(REACHED_BY_SOMETHING_ELSE)) {
    assert.ok(known.has(command), `'${command}' is excused (${why}) and the till has no such command`);
    assert.equal(
      asked.has(command),
      false,
      `'${command}' is excused as reached by something else, and a screen now runs it: take it ` +
        `off the list`,
    );
  }
});
