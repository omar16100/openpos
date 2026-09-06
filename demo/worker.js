// The till runs here. OPFS sync access handles exist only in a worker, which is
// why this file exists at all.
import init, { TillHandle } from './pkg/openpos_bindings.js';

const say = (m) => postMessage({ log: m });

async function openHandles(names) {
  const root = await navigator.storage.getDirectory();
  const handles = [];
  for (const name of names) {
    const file = await root.getFileHandle(name, { create: true });
    handles.push(await file.createSyncAccessHandle());
  }
  return handles;
}

self.onmessage = async (event) => {
  const { action } = event.data;
  let handles = [];
  try {
    await init();
    const names = TillHandle.fileNames();

    if (action === 'wipe') {
      const root = await navigator.storage.getDirectory();
      for (const name of names) {
        try { await root.removeEntry(name); } catch (_) { /* first run */ }
      }
      postMessage({ done: 'wiped' });
      return;
    }

    if (action === 'peek') {
      // Read the file with plain JavaScript, so what is on disk is established
      // independently of the Rust read path being debugged.
      const root = await navigator.storage.getDirectory();
      const fh = await root.getFileHandle('critical.log');
      const bytes = new Uint8Array(await (await fh.getFile()).arrayBuffer());
      const head = Array.from(bytes.slice(0, 8));
      const magic = String.fromCharCode(...bytes.slice(0, 4));
      postMessage({ done: `critical.log is ${bytes.length} bytes, first8=${JSON.stringify(head)}, magic="${magic}"` });
      return;
    }

    handles = await openHandles(names);
    say('sizes on open: ' + JSON.stringify(Array.from(TillHandle.fileSizes(handles))));
    say('rust read of critical.log: ' + TillHandle.peekCritical(handles));
    say('storage self test: ' + TillHandle.selfTest(handles));
    let till;
    try {
      till = TillHandle.openOpfs(handles, '0000000000000000000000002A', '00000000000000000000000007');
    } catch (e) {
      postMessage({ done: 'FAIL opening: ' + e.message });
      return;
    }

    if (action === 'sell') {
      till.applyItems(JSON.stringify([{
        id: '00000000000000000000000001', code: 'RICE5', name: 'Rice Miniket 5kg',
        price_minor: 43000, vat_bp: 1500, price_inclusive: false,
        barcodes: ['8690000000001'], on_hand_milli: 40000,
      }]));
      const scanned = JSON.parse(till.scan('8690000000001', 2000));
      say(`rang: total=${scanned.total_minor} error=${scanned.error}`);
      till.addCash(100000);
      const done = JSON.parse(till.checkout('00000000000000000000000384', Date.now()));
      say(`checkout error=${done.error} unsynced=${done.unsynced_sales}`);
      say('sizes after sale: ' + JSON.stringify(Array.from(TillHandle.fileSizes(handles))));
      for (const h of handles) h.close();
      postMessage({ done: `sold, unsynced=${done.unsynced_sales}` });
      return;
    }

    if (action === 'reopen') {
      const view = JSON.parse(till.view());
      say(`after reload: unsynced=${view.unsynced_sales} lines=${view.lines.length}`);
      for (const h of handles) h.close();
      postMessage({ done: `reopened, unsynced=${view.unsynced_sales}` });
      return;
    }
  } catch (e) {
    for (const h of handles) { try { h.close(); } catch (_) {} }
    postMessage({ done: 'THREW: ' + e });
  }
};
