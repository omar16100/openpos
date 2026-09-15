import { test } from 'node:test';
import assert from 'node:assert/strict';

import { keepAskingWhoIsThere, ASK_EVERY_MS, PATIENCE } from './still_someone_there.js';

/// A clock the test winds by hand, so twenty seconds of silence costs nothing.
function aClock() {
    let next = 1;
    const rounds = new Map();
    return {
        setInterval(work, everyMs) {
            const id = next++;
            rounds.set(id, { work, everyMs });
            return id;
        },
        clearInterval(id) {
            rounds.delete(id);
        },
        running: () => rounds.size,
        tick(times = 1) {
            for (let i = 0; i < times; i += 1) {
                for (const round of [...rounds.values()]) round.work();
            }
        },
    };
}

test('a page that answers keeps the shop’s files', () => {
    const timers = aClock();
    let letGo = 0;
    const watch = keepAskingWhoIsThere({
        ask: () => watch.answered(),
        letGo: () => {
            letGo += 1;
        },
        timers,
    });

    timers.tick(50);
    assert.equal(letGo, 0, 'a screen that is there never loses its till');
    assert.equal(timers.running(), 1, 'and it goes on being asked');
});

test('a worker whose page has gone lets the files go', () => {
    // The failure, found with every window closed: three tills still syncing to
    // the shop, still holding their stores, and the shop's device list saying
    // they had been reached seconds ago. Nobody was at any of them. The next
    // window was told the till was open in another window on this device, and
    // there was no other window to close.
    const timers = aClock();
    let letGo = 0;
    keepAskingWhoIsThere({
        ask: () => {},
        letGo: () => {
            letGo += 1;
        },
        timers,
    });

    timers.tick(PATIENCE);
    assert.equal(letGo, 0, 'not while there is still patience left');

    timers.tick(1);
    assert.equal(letGo, 1, 'and then the files are somebody else’s to take');
});

test('the files are let go once, not every round after', () => {
    const timers = aClock();
    let letGo = 0;
    keepAskingWhoIsThere({
        ask: () => {},
        letGo: () => {
            letGo += 1;
        },
        timers,
    });

    timers.tick(40);
    assert.equal(letGo, 1);
    assert.equal(timers.running(), 0, 'and nothing is left ticking behind it');
});

test('a screen that was busy and then answers is not counted against', () => {
    // A long task on the page's thread looks like silence for as long as it
    // runs. Losing a till's files under a cashier because the screen was busy
    // would be a worse bug than the one this fixes, so the count starts again
    // the moment anything is heard.
    const timers = aClock();
    let letGo = 0;
    const watch = keepAskingWhoIsThere({
        ask: () => {},
        letGo: () => {
            letGo += 1;
        },
        timers,
    });

    timers.tick(PATIENCE);
    watch.answered();
    timers.tick(PATIENCE);
    assert.equal(letGo, 0);
    assert.equal(watch.unanswered(), PATIENCE);
});

test('stopping ends the asking', () => {
    const timers = aClock();
    let letGo = 0;
    const watch = keepAskingWhoIsThere({
        ask: () => {},
        letGo: () => {
            letGo += 1;
        },
        timers,
    });

    watch.stop();
    timers.tick(50);
    assert.equal(letGo, 0, 'a till let go for its own reason is not let go twice');
});

test('the wait is long enough for a hidden tab and short enough for a queue', () => {
    // Both halves are load-bearing. Shorter and a screen doing a long piece of
    // work loses its till; longer and somebody who closed one window and opened
    // another stands at the counter waiting for a page that has gone.
    const silence = ASK_EVERY_MS * (PATIENCE + 1);
    assert.ok(silence >= 15_000, `${silence}ms is not long enough to be sure`);
    assert.ok(silence <= 30_000, `${silence}ms is too long to stand at a counter`);
});
