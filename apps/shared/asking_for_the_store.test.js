import { test } from 'node:test';
import assert from 'node:assert/strict';

import { answerWhoAsks, askForTheStore } from './asking_for_the_store.js';

/// Broadcast channels, as the browser has them: everybody on a name hears what
/// anybody else posts, and nobody hears themselves.
function aBrowser() {
  const byName = new Map();
  return (name) => {
    const here = byName.get(name) ?? [];
    const channel = {
      name,
      onmessage: null,
      postMessage(data) {
        for (const other of here) {
          if (other !== channel && other.onmessage) other.onmessage({ data });
        }
      },
      close() {
        const at = here.indexOf(channel);
        if (at >= 0) here.splice(at, 1);
      },
    };
    here.push(channel);
    byName.set(name, here);
    return channel;
  };
}

const now = () => ({ setTimeout: (run, ms) => setTimeout(run, ms) });

test('the window that has the shop gives it up when another window asks', async () => {
  const make = aBrowser();
  let letGoCalled = 0;
  answerWhoAsks('T1', { busy: () => false, letGo: async () => { letGoCalled += 1; }, make });

  const said = await askForTheStore('T1', { make, waitMs: 200, timers: now(), from: 'another window' });
  assert.equal(said.said, 'let_go');
  assert.equal(letGoCalled, 1, 'and it actually let go');
});

test('the files are free before the answer comes back', async () => {
  // The order is the whole point: a window told "let go" that then cannot open
  // the files is the failure this replaces, said differently.
  const make = aBrowser();
  let released = false;
  answerWhoAsks('T1', {
    busy: () => false,
    letGo: async () => {
      await new Promise((settle) => setTimeout(settle, 20));
      released = true;
    },
    make,
  });

  const said = await askForTheStore('T1', { make, waitMs: 500, timers: now(), from: 'another window' });
  assert.equal(said.said, 'let_go');
  assert.equal(released, true, 'the answer waited for the files');
});

test('a window in the middle of something keeps the shop and says so', async () => {
  const make = aBrowser();
  let letGoCalled = 0;
  answerWhoAsks('T1', { busy: () => 'counting', letGo: async () => { letGoCalled += 1; }, make });

  const said = await askForTheStore('T1', { make, waitMs: 200, timers: now(), from: 'another window' });
  assert.equal(said.said, 'busy');
  assert.equal(
    said.because,
    'counting',
    'and what it is in the middle of, because only it knows whether that is a sale or a count',
  );
  assert.equal(letGoCalled, 0, 'a basket rung and not paid for is not dropped');
});

test('nothing answers for a store held by a window that has gone', async () => {
  const make = aBrowser();
  const said = await askForTheStore('T1', { make, waitMs: 50, timers: now(), from: 'another window' });
  assert.equal(said.said, 'nobody');
});

test('a window that has stopped listening does not answer', async () => {
  const make = aBrowser();
  const stop = answerWhoAsks('T1', { busy: () => false, letGo: async () => {}, make });
  stop();

  const said = await askForTheStore('T1', { make, waitMs: 50, timers: now(), from: 'another window' });
  assert.equal(said.said, 'nobody');
});

test('a window holding a different till hears nothing', async () => {
  const make = aBrowser();
  let letGoCalled = 0;
  answerWhoAsks('T2', { busy: () => false, letGo: async () => { letGoCalled += 1; }, make });

  const said = await askForTheStore('T1', { make, waitMs: 50, timers: now(), from: 'another window' });
  assert.equal(said.said, 'nobody', 'two tills on one device are two stores');
  assert.equal(letGoCalled, 0);
});

test('an answer to somebody else’s asking is not taken', async () => {
  // Two windows asking at once, which is what happens when a shopkeeper presses
  // twice. Each hears only the answer to its own asking, so neither is told the
  // store is free on the strength of the other one being told.
  const make = aBrowser();
  const channel = make('openpos.store.T1');
  channel.onmessage = (event) => {
    if (event.data?.asking) channel.postMessage({ answering: 'somebody else', let_go: true });
  };

  const said = await askForTheStore('T1', { make, waitMs: 60, timers: now(), from: 'another window' });
  assert.equal(said.said, 'nobody');
});

test('a window does not answer its own asking', async () => {
  // A broadcast channel does not deliver back to the object that posted, which
  // is not the same as not delivering back to the window: each side holds its
  // own channel. So the window that pressed the button answered itself, gave up
  // a store it did not have, and told the person the shop had moved to another
  // window, which was the one they were looking at.
  const make = aBrowser();
  let letGoCalled = 0;
  answerWhoAsks('T1', { busy: () => false, letGo: async () => { letGoCalled += 1; }, make });

  const said = await askForTheStore('T1', { make, waitMs: 80, timers: now() });
  assert.equal(said.said, 'nobody', 'nobody else has it');
  assert.equal(letGoCalled, 0, 'and nothing was given up');
});
