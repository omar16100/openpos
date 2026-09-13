/// What an item still scans as after somebody corrects it.
///
/// Kept apart from `barcodes.js`, which is about reading a number off a box with
/// a camera and deciding whether to believe it. This is about the shop's own
/// record of which numbers belong to one thing on the shelf, which is a
/// different question asked on a different screen.
///
/// An item can have several barcodes and the screen that corrects one has a
/// single box. That is the right box: a shopkeeper correcting a number is
/// correcting the one on the label in front of them. What was wrong was what
/// happened to the others.
///
/// The form loaded the first barcode and saved that one alone, so an item with
/// two lost one every time anybody changed its price. Nothing said so, and
/// nothing could: it is not a failure, it is a field arriving shorter than it
/// left. The way a shop finds out is a cashier scanning the old box at the
/// counter and being told the shop has never heard of it, with a queue waiting
/// while somebody types thirteen digits in by hand.
///
/// Two barcodes on one item is ordinary rather than exotic: the same soap in a
/// box with an old label and a new one, and a spreadsheet brought in against an
/// item the shop already sells, which adds its number beside the one already
/// there rather than replacing it. The importer has always done that, so this
/// product has always been able to make an item that screen then broke.

/// The barcodes to save: the one in the box, then the others it already had.
///
/// The typed one first, because it is the one somebody is looking at and the
/// first is what the screen shows next time. Blanks dropped, because an empty
/// box means an item with no barcode rather than an item with an empty one.
/// Duplicates dropped, so typing a number the item already had further down its
/// list leaves one of it rather than two.
///
/// Nothing is trimmed off the others: they came from the shop's own records and
/// this is not the place to tidy them. The typed one is trimmed, because a space
/// at the end of it is somebody's keyboard rather than somebody's intention.
export function barcodesKept(typed, others = []) {
  const all = [typeof typed === 'string' ? typed.trim() : '', ...others];
  return all.filter((code, at) => code && all.indexOf(code) === at);
}
