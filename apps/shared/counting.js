/// A stock count that survives the screen it is being typed into.
///
/// Counting a shop means walking the shelves with a device, and a shop has more
/// items than fit on one screen. Until this existed the count was whatever was
/// in the boxes at the moment somebody pressed the button: a reload, a closed
/// tab or a flat battery took the morning with it, and nothing said how far
/// along it was.
///
/// So the sheet is written down as it is typed, keyed by shop, and it is the
/// thing the screen renders rather than the boxes being the truth. Filing it
/// sends the lines in batches and keeps whatever has not been accepted yet, so
/// a count interrupted half way through is a count that carries on rather than
/// one that starts again.
///
/// The rules live here rather than in the app because they are the part worth
/// testing: what a sheet contains, and what is left after a batch is filed. What
/// a typed quantity means is next door, because the till types them too.

import { milliFrom } from './quantity.js';

export { milliFrom };

/// Where one shop's sheet is kept. Per shop, because one browser can hold the
/// back office of two.
export function sheetKey(tenant) {
  return `openpos.admin.count.${tenant}`;
}

/// A sheet nobody has written in yet.
export function startSheet(startedAtMs) {
  return { started_at_ms: startedAtMs, lines: {} };
}

/// Write one shelf into the sheet, or take it out again.
///
/// An empty box is a shelf nobody counted, which is not the same as a shelf
/// found empty: `Number('')` is zero, and booking that would report every shelf
/// somebody cleared the box on as empty, which is the one wrong answer a count
/// can give that looks like a real finding. So an empty box removes the line.
///
/// The line id is minted once and kept. A count is idempotent on that id at the
/// server, so filing a batch whose reply was dropped costs nothing.
export function writeLine(sheet, itemId, typed, mintId) {
  const lines = { ...sheet.lines };
  if (String(typed).trim() === '') {
    delete lines[itemId];
    return { ...sheet, lines };
  }
  const held = lines[itemId];
  lines[itemId] = {
    id: held?.id ?? mintId(),
    typed: String(typed),
  };
  return { ...sheet, lines };
}

/// The lines that are ready to file: the ones whose quantity is a quantity.
///
/// A box holding something that is not a number stays in the sheet and is not
/// sent. It is somebody mid-keystroke, or a typo they will come back to, and
/// dropping it silently would be a shelf they believe they counted.
export function fileable(sheet) {
  return Object.entries(sheet.lines)
    .map(([itemId, line]) => ({
      item_id: itemId,
      id: line.id,
      qty_milli: milliFrom(line.typed),
    }))
    .filter((line) => line.qty_milli !== null);
}

/// What is not a quantity, so the screen can say which shelves are still wrong.
export function unusable(sheet) {
  return Object.entries(sheet.lines)
    .filter(([, line]) => milliFrom(line.typed) === null)
    .map(([itemId]) => itemId);
}

/// The sheet after a batch has been accepted: those shelves come out of it.
export function without(sheet, filed) {
  const done = new Set(filed.map((line) => line.item_id));
  const lines = {};
  for (const [itemId, line] of Object.entries(sheet.lines)) {
    if (!done.has(itemId)) lines[itemId] = line;
  }
  return { ...sheet, lines };
}

/// How far along it is, for a screen that has to say so.
export function summary(sheet) {
  const counted = fileable(sheet).length;
  const wrong = unusable(sheet).length;
  return { counted, wrong, total: counted + wrong };
}
