/// No screen decides what a role means.
///
/// The back office offers two choices and sends what the choice means, so the
/// choice is a rule. It held its own copy of that rule and the core held
/// another, and the two disagreed: the screen's cashier could open the drawer
/// and the core's could not, the screen's supervisor was capped at a fifth off
/// and the core's at everything. Every caller of the core's pair was a test, so
/// nothing a shop ran was inconsistent and nothing would have complained until
/// the first caller that was not a test.
///
/// A scan rather than a comment, because the copy that was there looked
/// perfectly reasonable sitting in the screen beside the dropdown it filled.
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

/// The flags a permission set is made of, as the core names them.
const FLAGS = [
  'may_override_price',
  'may_refund',
  'may_void_line',
  'may_authorise',
  'may_open_drawer',
  'may_close_shift',
  'max_discount_bp',
];

const SCREENS = ['../admin/src/App.svelte', '../till-web/src/App.svelte'];

test('no screen writes down what a role may do', () => {
  for (const path of SCREENS) {
    const screen = readFileSync(new URL(path, import.meta.url), 'utf8');
    for (const flag of FLAGS) {
      // Assigning a value to one of these is a screen deciding what somebody
      // may do. Reading one is fine and is how a list of people is shown.
      const assigned = new RegExp(`${flag}\\s*:\\s*(true|false|\\d)`);
      assert.equal(
        assigned.test(screen),
        false,
        `${path} sets ${flag} itself. What a role means is decided in core/src/auth.rs and asked ` +
          `for by name through rolesOffered(), or the shop gets two answers that disagree.`,
      );
    }
  }
});

test('the back office asks the core what each role means', () => {
  const screen = readFileSync(new URL('../admin/src/App.svelte', import.meta.url), 'utf8');
  assert.match(
    screen,
    /rolesOffered\(\)/,
    'the back office no longer asks the core what a role means, so something else is deciding',
  );
  // And refuses to save somebody before it knows. Sending no permissions adds
  // a person who may do nothing and looks on every screen like a cashier.
  assert.match(screen, /roles\[personRole\]/);
  assert.match(screen, /admin\.roles_not_ready/);
});
