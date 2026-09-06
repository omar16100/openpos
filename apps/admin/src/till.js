// The bridge to the worker, shared with every other app that drives the core.
//
// Only the worker itself is per-app: the bundler resolves this URL against the
// file it is written in, and each app's entry hands the core its own copy of
// the wasm under its own base path.
import { useWorker } from '../../shared/till.js';

useWorker(() => new Worker(new URL('./till.worker.js', import.meta.url), { type: 'module' }));

export * from '../../shared/till.js';
