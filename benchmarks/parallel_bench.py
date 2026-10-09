#!/usr/bin/env python3
"""Parallel-ingest benchmark: pyuppsala vs lxml on many-document parsing.

Measures the workload a federation aggregator actually has -- parse many
separate metadata documents -- under several execution models:

* sequential ``fromstring`` loop (single core, both backends);
* ``ThreadPoolExecutor`` + per-item ``fromstring`` (both backends -- note
  lxml ALSO releases the GIL during parse, so this column is measured, not
  assumed serial; pyuppsala's edge here is the faster parser, not the GIL);
* native ``pyuppsala.parse_many`` at 1/2/4/N threads (single FFI call for the
  whole batch, no Python thread overhead -- no lxml equivalent);
* optional ``fetch_many`` / ``fetch_and_parse_many`` against a local HTTP
  server, vs a ``requests.Session`` thread pool (run with ``--fetch``).

Usage:
    python benchmarks/parallel_bench.py [AGGREGATE.xml] [--reps N] [--fetch]

The corpus is the per-entity fragments of the given SAML aggregate (defaults
to the pyFF test aggregate used by etree_bench.py).
"""

import argparse
import concurrent.futures
import gc
import os
import statistics
import sys
import time

DEFAULT_AGGREGATE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "..",
    "pyFF",
    "src",
    "pyff",
    "test",
    "data",
    "metadata",
    "swamid-2.0-test.xml",
)


def load_fragments(path):
    """Split the aggregate into standalone per-entity XML documents."""
    from pyuppsala import etree as ET

    tree = ET.parse(path)
    root = tree.getroot()
    md = "{urn:oasis:names:tc:SAML:2.0:metadata}EntityDescriptor"
    frags = [
        ET.tostring(e, encoding="unicode") for e in root.iter(md)
    ]
    return frags


def timeit(fn, reps):
    """Best-of-N wall time (seconds) with GC disabled during the timing."""
    best = float("inf")
    for _ in range(reps):
        gc.collect()
        gc.disable()
        t0 = time.perf_counter()
        fn()
        dt = time.perf_counter() - t0
        gc.enable()
        best = min(best, dt)
    return best


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("aggregate", nargs="?", default=DEFAULT_AGGREGATE)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--fetch", action="store_true", help="include the HTTP fetch benchmark")
    args = ap.parse_args()

    import pyuppsala
    from pyuppsala import etree as PUET

    try:
        import lxml.etree as LXET
    except ImportError:
        LXET = None

    frags = load_fragments(args.aggregate)
    frags_b = [f.encode() for f in frags]
    total_mb = sum(len(f) for f in frags_b) / 1e6
    ncpu = os.cpu_count() or 1
    print(f"corpus: {len(frags)} entity documents, {total_mb:.1f} MB total, {ncpu} CPUs\n")

    rows = []

    def add(name, dt, base=None):
        rows.append((name, dt, base))

    # Sequential loops.
    seq_pu = timeit(lambda: [PUET.fromstring(f) for f in frags_b], args.reps)
    add("sequential fromstring (pyuppsala)", seq_pu)
    seq_lx = None
    if LXET is not None:
        seq_lx = timeit(lambda: [LXET.fromstring(f) for f in frags_b], args.reps)
        add("sequential fromstring (lxml)", seq_lx)

    # Python-thread pools (lxml releases the GIL during parse too).
    for workers in (2, 4):
        if workers > ncpu:
            continue

        def pool_pu(w=workers):
            with concurrent.futures.ThreadPoolExecutor(w) as ex:
                list(ex.map(PUET.fromstring, frags_b))

        add(f"ThreadPool({workers}) fromstring (pyuppsala)", timeit(pool_pu, args.reps))
        if LXET is not None:

            def pool_lx(w=workers):
                with concurrent.futures.ThreadPoolExecutor(w) as ex:
                    list(ex.map(LXET.fromstring, frags_b))

            add(f"ThreadPool({workers}) fromstring (lxml)", timeit(pool_lx, args.reps))

    # Native batch (no lxml equivalent).
    for threads in (1, 2, 4, None):
        if threads is not None and threads > ncpu:
            continue
        label = threads if threads is not None else f"auto={ncpu}"

        def batch(t=threads):
            pyuppsala.parse_many(frags, max_threads=t)

        add(f"parse_many(max_threads={label})", timeit(batch, args.reps))

    def batch_etree():
        PUET.fromstring_many(frags)

    add("etree.fromstring_many(auto)", timeit(batch_etree, args.reps))

    print(f"{'model':<44} {'wall':>10}   vs seq-pyuppsala")
    print("-" * 75)
    for name, dt, _ in rows:
        speedup = seq_pu / dt if dt else float("inf")
        print(f"{name:<44} {dt * 1000:>8.1f}ms   {speedup:>5.2f}x")

    if args.fetch and hasattr(pyuppsala, "fetch_many"):
        run_fetch_bench(frags_b, args.reps)


def run_fetch_bench(frags_b, reps):
    """fetch_many vs a requests thread pool against a local HTTP server."""
    import http.server
    import threading

    import pyuppsala

    bodies = {f"/e{i}.xml": b for i, b in enumerate(frags_b)}

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = bodies.get(self.path)
            if body is None:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/samlmetadata+xml")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    urls = [f"{base}{p}" for p in bodies]

    print(f"\nfetch benchmark: {len(urls)} URLs from a local server")
    t = timeit(lambda: pyuppsala.fetch_many(urls, max_threads=8), reps)
    print(f"{'fetch_many(8)':<44} {t * 1000:>8.1f}ms")
    t = timeit(lambda: pyuppsala.fetch_and_parse_many(urls, max_threads=8), reps)
    print(f"{'fetch_and_parse_many(8)':<44} {t * 1000:>8.1f}ms")

    try:
        import requests

        def rq():
            with requests.Session() as s:
                with concurrent.futures.ThreadPoolExecutor(8) as ex:
                    list(ex.map(lambda u: s.get(u).content, urls))

        t = timeit(rq, reps)
        print(f"{'requests Session + ThreadPool(8)':<44} {t * 1000:>8.1f}ms")
    except ImportError:
        print("requests not installed; skipping the comparison row")
    srv.shutdown()


if __name__ == "__main__":
    sys.exit(main())
