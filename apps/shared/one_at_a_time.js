/// One round at a time, for work a timer starts and does not wait for.
///
/// The till's sync loop is a `setInterval`, and an interval does not care
/// whether the last thing it started has finished. On a shop's line it often
/// has not: a post that takes longer than the interval, which is most of them
/// when the connection is bad, means the next tick starts a second round on top
/// of the first.
///
/// What that costs is not theoretical. A till asks the shop for a block of five
/// hundred receipt numbers; the reply is slow; the next tick asks again, because
/// the first grant has not been applied yet. The till keeps the block it is
/// using and one in reserve, so the block in the middle is stranded and the
/// shop's printed receipt numbers jump by five hundred with nothing to explain
/// it. Every other step doubles up the same way, which is a device talking over
/// itself to a shop that is already struggling to answer.
///
/// Dropped rather than queued. The next tick is a moment away, and a queue of
/// rounds waiting on a slow shop is the pile-up this exists to prevent.

/// Wrap `work` so it runs alone.
///
/// The wrapper hands back what `work` returned, or `skipped` when a round was
/// already in the air. Failures travel: a round that throws throws here too, and
/// the door is open again either way.
export function oneAtATime(work, { skipped = undefined } = {}) {
  let inTheAir = false;
  return async (...given) => {
    if (inTheAir) return skipped;
    inTheAir = true;
    try {
      return await work(...given);
    } finally {
      // However it went. A round that threw and left this set would stop the
      // till syncing for good, silently, which is worse than anything this was
      // put here to prevent.
      inTheAir = false;
    }
  };
}
