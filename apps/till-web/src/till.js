// The bridge to the worker, shared with every other app that drives the core.
//
// Only the worker itself is per-app: the bundler resolves this URL against the
// file it is written in, and each app's entry hands the core its own copy of
// the wasm under its own base path.
import { letGoOnTheWayOut, useWorker } from '../../shared/till.js';

useWorker(() => new Worker(new URL('./till.worker.js', import.meta.url), { type: 'module' }));

// Set up where the worker is, because the two belong together: this page holds
// the files through that worker, and a page that leaves without saying so leaves
// them held for a window that no longer exists.
letGoOnTheWayOut();

export * from '../../shared/till.js';
