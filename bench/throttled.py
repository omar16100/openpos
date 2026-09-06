"""Same hot path, CPU-throttled to stand in for a cheap Android tablet.

Playwright cannot throttle directly, so this drives CDP Emulation.setCPUThrottlingRate.
6x is a reasonable proxy for a mid-range tablet, 12x for a low-end 2 GB device.
"""
import functools
import http.server
import socketserver
import threading
from pathlib import Path

from playwright.sync_api import sync_playwright

BENCH_DIR = Path(__file__).parent


def run(rate: int, port: int) -> dict:
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        cdp = page.context.new_cdp_session(page)
        cdp.send("Emulation.setCPUThrottlingRate", {"rate": rate})
        page.goto(f"http://127.0.0.1:{port}/hotpath_page.html")
        result = page.evaluate("() => window.bench(20000)")
        browser.close()
    return result


def main() -> None:
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(BENCH_DIR))
    with socketserver.TCPServer(("127.0.0.1", 0), handler) as server:
        port = server.server_address[1]
        threading.Thread(target=server.serve_forever, daemon=True).start()
        print(f"{'cpu':>6} {'snapshot hydrate':>18} {'getAll 20k':>12} {'20k row write':>15} {'map get':>10}")
        for rate in (1, 6, 12):
            r = run(rate, port)
            print(
                f"{rate:>5}x {r['snapshot_load_and_index_ms']:>15.1f} ms "
                f"{r['idb_getall_ms']:>9.1f} ms {r['idb_bulk_write_ms']:>12.1f} ms "
                f"{r['map_lookup_us']:>7.3f} us"
            )
        server.shutdown()


if __name__ == "__main__":
    main()
