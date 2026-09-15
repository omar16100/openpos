/// Reading a month's selling by the words a shop sorts its shelves by.
///
/// The list of what sold is one long list of items, and a shop with eight
/// hundred of them reads the top twenty and learns nothing about whether the
/// rice moved. Grouping is the whole of it: the shop's own words, in order,
/// with the things nobody has sorted at the end rather than hidden.

/// The heading for goods the shop has not put under anything yet.
///
/// Named rather than left blank, because a blank heading reads as a fault in
/// the screen. Most of a catalogue is unsorted on the first day and that is
/// nobody's mistake.
export const UNSORTED = 'Not sorted yet';

/// Group rows of what sold under the category of each item.
///
/// `kinds` is what the shop calls each item, by item id. Anything missing from
/// it, or sorted under nothing, goes under `UNSORTED`.
///
/// Row order inside a group is left exactly as it arrived, because the caller
/// sorted it by what moved most and that is the order an owner reads. Quantities
/// are deliberately not summed across a group: a kilo of rice and a bar of soap
/// are not two of anything.
export function groupSold(rows, kinds) {
  const groups = new Map();
  for (const row of rows ?? []) {
    const said = (kinds?.[row.item] ?? '').trim();
    const kind = said === '' ? UNSORTED : said;
    const held = groups.get(kind);
    if (held) {
      held.push(row);
    } else {
      groups.set(kind, [row]);
    }
  }
  return [...groups.entries()]
    .map(([kind, held]) => ({ kind, rows: held }))
    .sort((one, two) => {
      // The unsorted go last whatever they are called, so a shop reading its
      // own words is not interrupted by the pile it has not got to yet.
      if (one.kind === UNSORTED) return 1;
      if (two.kind === UNSORTED) return -1;
      return one.kind.localeCompare(two.kind);
    });
}
