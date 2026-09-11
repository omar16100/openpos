/// Reading a barcode, and knowing when not to believe it.
///
/// A shop here has two ways of getting a number off a box. The usual one is a
/// scanner on a wire, which types the digits and presses Enter: nothing in this
/// file is involved, because to the screen that is somebody typing fast. The
/// other is the camera in the tablet, for a shop that has no scanner, for a
/// second counter on a market day, and for the back office where nobody is
/// going to buy a scanner at all.
///
/// A camera read is not a scanner read. A scanner sees the label straight on,
/// at a fixed distance, under its own light; a camera is held at an angle by
/// somebody with a customer waiting, and it will hand back a number it is not
/// sure about. So two things happen here that a wire scanner does not need:
/// the check digit is worked out, and the same code has to be read twice in a
/// row before the till hears about it.
///
/// The decoding itself is the browser's, and this file never sees a pixel:
/// that keeps it testable, keeps it small, and keeps the app working with the
/// internet down, because nothing has to be fetched to read a barcode.

/// The symbologies a shop here actually has on its shelves.
///
/// EAN-13 on anything that came through a distributor, EAN-8 on small packets,
/// UPC on imports, Code 128 and ITF-14 on cartons a wholesaler labelled
/// themselves. QR is deliberately absent: a QR code on a counter is a payment,
/// not an item, and reading one into the basket would ring whatever number a
/// customer's phone was showing.
export const SYMBOLOGIES = ['ean_13', 'ean_8', 'upc_a', 'upc_e', 'code_128', 'itf'];

/// Whether the browser can read a barcode at all.
///
/// `BarcodeDetector` is in Chrome and in the Android WebView, which is what the
/// shops this is for are running. Where it is missing, the screen says so and
/// the scanner box is still there: a till that hides the camera button is
/// better than one that opens a dead window onto a customer.
export function canReadBarcodes(where = globalThis) {
  return typeof where.BarcodeDetector === 'function';
}

/// The check digit of an EAN-13, EAN-8, UPC-A or UPC-E number.
///
/// Every one of them is the same sum: the digits before the last, weighted
/// three and one from the right, summed, and taken up to the next ten.
///
/// Worked out here rather than trusted from the reader because the reader is
/// looking at a label through a tablet's camera at an angle. A misread digit
/// that happens to keep the check digit right is possible and rare; one that
/// does not is common and free to catch, and the cost of not catching it is a
/// customer charged for something they are not holding.
export function checkDigitOf(digits) {
  if (typeof digits !== 'string' || !/^\d+$/.test(digits)) return null;
  let sum = 0;
  // From the right, because the weights are anchored at the check digit and
  // the numbers are different lengths.
  for (let at = 0; at < digits.length; at += 1) {
    const digit = Number(digits[digits.length - 1 - at]);
    sum += at % 2 === 0 ? digit * 3 : digit;
  }
  return (10 - (sum % 10)) % 10;
}

/// The twelve digits a six-digit UPC-E stands for.
///
/// UPC-E is a UPC-A with runs of zeros squeezed out, and the last digit printed
/// on it is the check digit of the *expanded* number, not of the eight digits
/// as they stand. Checking it as though it were an EAN-8 rejects real labels: a
/// shop whose imported tins would not scan, with nothing on the screen to say
/// why.
///
/// The rule is the sixth digit of the body, which says where the zeros came
/// out. Written as the standard writes it, because a shorter version of this is
/// a version that is wrong for one of the six cases.
function expandedFromUpcE(code) {
  // A leading number system of 0 or 1, six digits, and a check digit.
  if (!/^[01]\d{7}$/.test(code)) return null;
  const system = code[0];
  const body = code.slice(1, 7);
  const check = code[7];
  const [a, b, c, d, e, f] = body;
  let middle;
  switch (f) {
    case '0':
    case '1':
    case '2':
      middle = `${a}${b}${f}0000${c}${d}${e}`;
      break;
    case '3':
      middle = `${a}${b}${c}00000${d}${e}`;
      break;
    case '4':
      middle = `${a}${b}${c}${d}00000${e}`;
      break;
    default:
      middle = `${a}${b}${c}${d}${e}0000${f}`;
      break;
  }
  return `${system}${middle}${check}`;
}

/// Whether a number checks out against its own last digit.
///
/// Every length that carries one: EAN-8, UPC-E once it is expanded, UPC-A,
/// EAN-13 and the fourteen digits of a carton's ITF-14, which is the same
/// alternating sum as the rest. Code 128 carries no check digit a reader can be
/// asked about here, so a code of another length is handed over as it was read:
/// refusing it would turn "this shop's own carton labels do not scan" into a
/// defect nobody can explain.
export function checksOut(code) {
  if (typeof code !== 'string' || !/^\d+$/.test(code)) return true;
  // Eight digits are either an EAN-8 or a UPC-E, and the two are checked
  // differently. Whichever it is, one of them has to be right: a real EAN-8
  // does not read as a valid expanded UPC-E by accident, and the alternative
  // was rejecting every imported tin in the shop.
  if (code.length === 8) {
    const expanded = expandedFromUpcE(code);
    return isRight(code) || (expanded !== null && isRight(expanded));
  }
  if (![12, 13, 14].includes(code.length)) return true;
  return isRight(code);
}

/// The last digit against the ones before it.
function isRight(code) {
  const body = code.slice(0, -1);
  const said = Number(code[code.length - 1]);
  return checkDigitOf(body) === said;
}

/// A twelve-digit UPC-A written as the thirteen digits a catalogue holds.
///
/// The same article carries both on different boxes, and a shop that files the
/// imported tin under twelve digits and scans thirteen has two items where it
/// has one. The leading zero is the whole of the difference.
export function asThirteen(code) {
  if (typeof code !== 'string' || !/^\d{12}$/.test(code)) return code;
  return `0${code}`;
}

/// What a camera has read, and whether the till should be told yet.
///
/// Two readings of the same code in a row, because one frame is a guess. The
/// second is nearly free: a camera gives twenty or thirty frames a second, so
/// waiting for agreement costs a fraction of a second and takes out the class
/// of misread that a check digit cannot.
///
/// `seen` is what the last call returned, or null the first time. It comes back
/// as `{ seen, ring }`: `ring` is the code to send, and is null until two
/// readings agree and the code checks out.
export function whatWasRead(seen, reading) {
  const code = typeof reading === 'string' ? reading.trim() : '';
  if (!code) return { seen: null, ring: null };
  if (!checksOut(code)) {
    // Read again rather than rung. A number that fails its own check digit is
    // a misread, and the customer is holding something else.
    return { seen: null, ring: null };
  }
  const code13 = asThirteen(code);
  if (seen === code13) return { seen: code13, ring: code13 };
  return { seen: code13, ring: null };
}
