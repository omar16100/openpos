"""Measure the till hot path in a real browser engine.

Desktop Chromium numbers. A cheap Android tablet runs JS and IndexedDB roughly
5 to 15 times slower, so read these as a lower bound and apply the multiplier.
"""
import functools
import http.server
import socketserver
import threading
from pathlib import Path

from playwright.sync_api import sync_playwright

BENCH_DIR = Path(__file__).parent


def fmt(value):
    if isinstance(value, (int, float)):
        return f"{value:.3f}" if value < 1 else f"{value:.1f}"
    return str(value)


def main() -> None:
    # IndexedDB is denied on about:blank, so the harness needs a real origin.
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(BENCH_DIR))
    with socketserver.TCPServer(("127.0.0.1", 0), handler) as server:
        port = server.server_address[1]
        threading.Thread(target=server.serve_forever, daemon=True).start()
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page()
            page.goto(f"http://127.0.0.1:{port}/hotpath_page.html")
            r = page.evaluate("() => window.bench(20000)")
            browser.close()
        server.shutdown()

    print(f"catalogue: 20,000 items, packed {r['packed_bytes']/1e6:.1f} MB json")
    print(f"  idb bulk write 20k rows     {fmt(r['idb_bulk_write_ms']):>9} ms")
    print(f"  idb getAll 20k rows         {fmt(r['idb_getall_ms']):>9} ms  ({r['idb_getall_rows']} rows)")
    print(f"  snapshot blob load + index  {fmt(r['snapshot_load_and_index_ms']):>9} ms  ({r['snapshot_rows']} rows)")
    print("barcode lookup:")
    print(f"  idb index get p50           {fmt(r['idb_lookup_p50_ms']):>9} ms")
    print(f"  idb index get p95           {fmt(r['idb_lookup_p95_ms']):>9} ms")
    print(f"  in-memory Map.get           {fmt(r['map_lookup_us']):>9} us")
    print("ticket durability (12 lines):")
    print(f"  idb commit relaxed p50      {fmt(r['ticket_commit_relaxed_p50_ms']):>9} ms")
    print(f"  idb commit relaxed p95      {fmt(r['ticket_commit_relaxed_p95_ms']):>9} ms")
    print(f"  idb commit strict  p50      {fmt(r['ticket_commit_strict_p50_ms']):>9} ms")
    print(f"  idb commit strict  p95      {fmt(r['ticket_commit_strict_p95_ms']):>9} ms")


if __name__ == "__main__":
    main()
