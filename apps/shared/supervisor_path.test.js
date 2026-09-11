/// Every command a supervisor can allow is asked for through the path that
/// fetches one.
///
/// The core answers a refusal with the action a supervisor would have to allow,
/// the view carries it, and the screen has a panel that offers the supervisors
/// by name and takes a PIN. All of that worked. What decided whether a
/// shopkeeper ever saw it was which helper the screen happened to call:
/// `attemptWithOverride` keeps the refused work and offers the panel, and
/// `attempt` shows the sentence and drops it.
///
/// Closing the drawer went through the plain one. A cashier may not close a
/// drawer and counting one is the last thing they do at the end of a shift, so
/// the screen said no and offered nothing: the count was retyped by a
/// supervisor who had to sign in, and the shop's record of who counted the
/// drawer said the supervisor. The same was true of opening the drawer, moving
/// cash, and the scan that follows writing an item down at the till.
///
/// So it is a scan rather than a habit. A command that can be refused on a
/// permission has to be asked for through the path that can do something about
/// it.
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

/// The commands the core can refuse with "a supervisor would have to allow
/// this", by the name the screen runs them under.
///
/// Taken from what the core maps in `blocked_by`: a discount and a price over
/// the catalogue's are refused by the cart's ceilings, the shelf and the credit
/// cap by the till's own rules, and everything else by the permission check.
/// Kept here as the list of commands rather than of actions, because what this
/// file can see is the call.
const CAN_NEED_A_SUPERVISOR = [
  'set_line_discount',
  'take_off_line',
  'set_ticket_discount',
  'take_off_ticket',
  'set_unit_price',
  'scan',
  'add_tender',
  'start_refund',
  'remove_line',
  'set_qty',
  'open_drawer',
  'move_cash',
  'close_shift',
];

const SCREEN = new URL('../till-web/src/App.svelte', import.meta.url);

/// Which helper this `op` is run through, everywhere it is run.
function askedThrough(source, op) {
  const ways = [];
  for (const found of source.matchAll(new RegExp(`op: '${op}'`, 'g'))) {
    // Back to the nearest `attempt(` or `attemptWithOverride(` before it. The
    // call is written across several lines, so this reads backwards rather
    // than matching a single line.
    const before = source.slice(0, found.index);
    const plain = before.lastIndexOf('attempt(');
    const withOne = before.lastIndexOf('attemptWithOverride(');
    ways.push(withOne > plain ? 'override' : 'plain');
  }
  return ways;
}

test('every command a supervisor can allow is asked for through the path that fetches one', () => {
  const source = readFileSync(SCREEN, 'utf8');
  for (const op of CAN_NEED_A_SUPERVISOR) {
    const ways = askedThrough(source, op);
    assert.ok(ways.length > 0, `${op} is not run by the till screen at all`);
    for (const way of ways) {
      assert.equal(
        way,
        'override',
        `${op} is run through attempt() rather than attemptWithOverride(), so a refusal that ` +
          `names a supervisor shows the sentence and offers nobody. The shop's way round that is ` +
          `to sign the cashier out and a supervisor in, which puts the sale, or the drawer count, ` +
          `under the wrong name.`,
      );
    }
  }
});
