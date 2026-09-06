// This app's worker: three lines, and they are the whole of what differs
// between the till and the back office.
//
// The wasm lives under this app's own base path, and the bundler rewrites that
// path per app, so this import cannot move into the shared file. Everything
// else did.
import init, { TillHandle } from '../public/pkg/openpos_bindings.js';
import { start } from '../../shared/till.worker.js';

start(init, TillHandle);
