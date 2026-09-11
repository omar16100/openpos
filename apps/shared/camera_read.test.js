import { test } from 'node:test';
import assert from 'node:assert/strict';

import { CANNOT_READ_HERE, WOULD_NOT_OPEN, readFromCamera } from './camera_read.js';

/// A camera, a decoder and a picture element, none of them real.
///
/// The loop is driven by a timer here rather than by the browser's frame
/// callback, which is what a tablet without one falls back to anyway.
function aCamera(readings) {
  const stopped = [];
  const video = {
    srcObject: null,
    play: async () => {},
    // No requestVideoFrameCallback, so the loop uses the timer path.
  };
  let at = 0;
  const detector = class {
    constructor(opts) {
      this.formats = opts?.formats;
    }
    async detect() {
      const one = readings[Math.min(at, readings.length - 1)];
      at += 1;
      return one === null ? [] : [{ rawValue: one }];
    }
  };
  const media = {
    getUserMedia: async () => ({ getTracks: () => [{ stop: () => stopped.push(true) }] }),
  };
  return { video, detector, media, stopped, reads: () => at };
}

/// Wait for the loop, which is on a 120 ms timer between frames.
function after(ms) {
  return new Promise((settle) => setTimeout(settle, ms));
}

test('a code read twice is handed over once, and the camera is shut first', async () => {
  const right = '4006381333931';
  const camera = aCamera([right, right, right, right]);
  const rung = [];
  await readFromCamera({
    video: camera.video,
    onCode: (code) => rung.push(code),
    detector: camera.detector,
    media: camera.media,
  });
  await after(400);
  assert.deepEqual(rung, [right], 'handed over once');
  assert.equal(camera.stopped.length, 1, 'and the camera is off');
  assert.equal(camera.video.srcObject, null, 'and the picture is let go');
});

test('one reading is not enough', async () => {
  const camera = aCamera(['4006381333931', null, null, null]);
  const rung = [];
  const held = await readFromCamera({
    video: camera.video,
    onCode: (code) => rung.push(code),
    detector: camera.detector,
    media: camera.media,
  });
  await after(400);
  assert.deepEqual(rung, [], 'one frame is a guess');
  held.stop();
});

test('a browser with no reader says so and opens nothing', async () => {
  const camera = aCamera(['4006381333931']);
  const trouble = [];
  const held = await readFromCamera({
    video: camera.video,
    onCode: () => assert.fail('nothing should be read'),
    onTrouble: (why) => trouble.push(why),
    detector: undefined,
    media: camera.media,
  });
  assert.deepEqual(trouble, [CANNOT_READ_HERE]);
  assert.equal(camera.video.srcObject, null, 'the camera was never asked for');
  held.stop();
});

test('a camera that will not open says so', async () => {
  const camera = aCamera(['4006381333931']);
  const trouble = [];
  await readFromCamera({
    video: camera.video,
    onCode: () => assert.fail('nothing should be read'),
    onTrouble: (why) => trouble.push(why),
    detector: camera.detector,
    media: { getUserMedia: async () => { throw new Error('refused'); } },
  });
  assert.deepEqual(trouble, [WOULD_NOT_OPEN]);
});

test('stopping it stops the reading', async () => {
  const camera = aCamera([null, null, null, null, null, null, null, null]);
  const held = await readFromCamera({
    video: camera.video,
    onCode: () => assert.fail('nothing to read'),
    detector: camera.detector,
    media: camera.media,
  });
  await after(150);
  const byThen = camera.reads();
  held.stop();
  await after(400);
  assert.ok(camera.reads() <= byThen + 1, 'no frames are looked at after it is stopped');
  assert.equal(camera.stopped.length, 1, 'and the camera is off');
});

test('a misread never rings, however often it is read', async () => {
  // The same label misread by one digit, over and over: the check digit is
  // what stops it, and a consistent misread is exactly the case two readings
  // cannot catch on their own.
  const camera = aCamera(['4006381333932', '4006381333932', '4006381333932']);
  const rung = [];
  const held = await readFromCamera({
    video: camera.video,
    onCode: (code) => rung.push(code),
    detector: camera.detector,
    media: camera.media,
  });
  await after(400);
  assert.deepEqual(rung, []);
  held.stop();
});

test('a camera kept looking hands one label over once, and the next one after it', async () => {
  const first = '4006381333931';
  const second = '5901234123457';
  // A label held in the frame is read thirty times a second. Somebody counting
  // a shelf types a number and moves on; the label they are standing in front
  // of must not arrive again while they do it.
  const camera = aCamera([first, first, first, first, second, second, second]);
  const read = [];
  const held = await readFromCamera({
    video: camera.video,
    keepLooking: true,
    onCode: (code) => read.push(code),
    detector: camera.detector,
    media: camera.media,
  });
  await after(1200);
  held.stop();
  assert.deepEqual(read, [first, second], 'each label once, in the order they were read');
  assert.equal(camera.stopped.length, 1, 'and the camera only stops when it is told to');
});

test('a picture that never starts playing does not hold the camera open', async () => {
  // What a second press produces: the first press is still asking for the
  // camera, the element is taken off the screen, and a video that is not on
  // the screen never begins playing. Waiting for it meant this never returned,
  // so the caller never got the handle, and the handle is the only way to let
  // the camera go.
  const camera = aCamera(['4006381333931']);
  camera.video.play = () => new Promise(() => {});
  const held = await Promise.race([
    readFromCamera({
      video: camera.video,
      onCode: () => {},
      detector: camera.detector,
      media: camera.media,
    }),
    after(800).then(() => 'never came back'),
  ]);
  assert.notEqual(held, 'never came back', 'the caller has to get its handle');
  held.stop();
  assert.equal(camera.stopped.length, 1, 'and stopping it lets the camera go');
});

test('a reader that will not be built lets the camera go', async () => {
  // Some browsers have the name and refuse the formats. Throwing here used to
  // leave the camera running with nobody holding the only thing that stops it.
  const camera = aCamera(['4006381333931']);
  const trouble = [];
  const held = await readFromCamera({
    video: camera.video,
    onCode: () => assert.fail('nothing should be read'),
    onTrouble: (why) => trouble.push(why),
    detector: class { constructor() { throw new Error('not these formats'); } },
    media: camera.media,
  });
  assert.deepEqual(trouble, [CANNOT_READ_HERE], 'and it says so in the shop\'s own words');
  assert.equal(camera.stopped.length, 1, 'the camera is let go');
  held.stop();
});
