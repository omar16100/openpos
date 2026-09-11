/// The loop that reads a barcode off a camera, without any screen in it.
///
/// Two screens want this: the till, where a read rings the goods, and the back
/// office, where it fills in the barcode box on an item nobody has a scanner
/// for. One loop, because two would be two answers to "when do we believe it",
/// and the whole point of reading twice is that the answer is the same both
/// times.
///
/// Nothing here touches the DOM it was not handed. The camera, the decoder and
/// the way the next frame is asked for all arrive as arguments, which is what
/// lets a test drive the whole loop with no browser in the room.

import { SYMBOLOGIES, whatWasRead } from './barcodes.js';

/// What went wrong, as something a screen can turn into its own words.
///
/// Codes rather than sentences, for the reason every refusal here carries one:
/// a screen that matched on English would go quiet the day it was reworded, or
/// the day the shop switched to Bangla.
export const CANNOT_READ_HERE = 'camera.not-here';
export const WOULD_NOT_OPEN = 'camera.would-not-open';

/// Open the camera and read until something is believed.
///
/// `onCode` is called once, with the code, and the camera is shut before it is:
/// a reader left running after the thing it was looking for has been found is
/// a light on the counter and a flat battery by the afternoon.
///
/// Returns a handle with `stop()`, which is safe to call twice and safe to call
/// after a read.
export async function readFromCamera({
  video,
  onCode,
  onTrouble,
  /// Whether to keep reading after one has been handed over.
  ///
  /// The till stops: a scan rings the goods and the camera has done its job.
  /// Somebody counting a shelf does not stop, because the next thing they do is
  /// the next shelf, and a camera they have to press a button to reopen between
  /// every item is a camera nobody uses for a shop of eight hundred lines.
  ///
  /// A label held in the frame is read thirty times a second, so what is handed
  /// over once is not handed over again until something else has been read.
  keepLooking = false,
  // The browser's own pieces, handed in so a test can stand in for them.
  detector = globalThis.BarcodeDetector,
  media = globalThis.navigator?.mediaDevices,
}) {
  const idle = { stop: () => {} };
  if (typeof detector !== 'function') {
    onTrouble?.(CANNOT_READ_HERE);
    return idle;
  }
  let stream;
  try {
    // The back camera, which is the one pointed at the goods. A preference
    // rather than a demand, so a laptop with one camera still opens.
    stream = await media.getUserMedia({ video: { facingMode: 'environment' } });
  } catch {
    // Refused, or there is no camera at all. One answer for both, because what
    // the person does about it is the same.
    onTrouble?.(WOULD_NOT_OPEN);
    return idle;
  }

  let reader;
  try {
    reader = new detector({ formats: SYMBOLOGIES });
  } catch {
    // A browser that has the name and cannot do the job: some builds refuse the
    // formats, and one that throws here would leave the camera running with
    // nobody holding the only thing that can stop it. The light stays on and
    // the shop has nothing to press.
    for (const track of stream?.getTracks() ?? []) track.stop();
    onTrouble?.(CANNOT_READ_HERE);
    return idle;
  }
  let seen = null;
  /// The last code handed over, so a label still sitting in the frame is not
  /// handed over again on the next frame, and the one after that.
  let handed = null;
  let reading = true;

  const stop = () => {
    reading = false;
    if (video) video.srcObject = null;
    for (const track of stream?.getTracks() ?? []) track.stop();
    stream = null;
  };

  if (video) {
    video.srcObject = stream;
    // Started rather than waited for. A picture element that has been taken off
    // the screen never begins playing, so a promise that resolves when playback
    // starts never settles, and waiting for it here meant this function never
    // returned: the caller never got the handle, and the only way to let the
    // camera go is the handle. A person who pressed the button twice because
    // the first press looked like nothing was left with a camera running and
    // no way to stop it but a reload.
    //
    // Nothing below needs playback to have started. The reader is handed the
    // element, and the element plays when it is on the screen because it says
    // so in the markup.
    void video.play?.().catch(() => {});
  }

  const next = () => {
    if (!reading || !video) return;
    // The browser's own frame callback where there is one, because a timer
    // reads the same frame twice on a slow tablet: two readings that agree
    // about a picture nobody looked at twice.
    if (video.requestVideoFrameCallback) video.requestVideoFrameCallback(look);
    else setTimeout(look, 120);
  };

  const look = async () => {
    if (!reading || !video) return;
    try {
      for (const one of await reader.detect(video)) {
        const read = whatWasRead(seen, one.rawValue);
        seen = read.seen;
        if (!read.ring) continue;
        if (read.ring === handed) continue;
        handed = read.ring;
        if (!keepLooking) {
          stop();
          onCode(read.ring);
          return;
        }
        // Kept open. What was read is handed over and the frames go on
        // arriving, so the next shelf is the next thing that happens.
        onCode(read.ring);
      }
    } catch {
      // A frame the reader could not look at. The next one is a fiftieth of a
      // second away, so this is not worth saying anything about.
    }
    next();
  };

  next();
  return { stop };
}
