// Turning the core's integers into something a person reads.
//
// Formatting only. The arithmetic already happened in Rust, and a division here
// that rounded differently would put a figure on screen that the receipt does
// not agree with.

/// Minor units to taka. Two decimals always: a price that shows as 43 when it
/// means 43.00 reads as a different price at a glance.
export function money(minor) {
  const negative = minor < 0;
  const whole = Math.trunc(Math.abs(minor) / 100);
  const part = String(Math.abs(minor) % 100).padStart(2, '0');
  return `${negative ? '-' : ''}${whole.toLocaleString('en-BD')}.${part}`;
}

/// Thousandths to a quantity. Trailing zeros are dropped, because most of a
/// shop's lines are whole units and "2" is easier to check than "2.000".
export function qty(milli) {
  const value = milli / 1000;
  return Number.isInteger(value) ? String(value) : value.toFixed(3).replace(/0+$/, '');
}
