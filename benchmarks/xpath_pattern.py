"""pyuppsala XPath-pattern measurement (compile-once target).

Measures the compiled ``XPath`` reuse pattern against per-call compilation on
a SAML metadata aggregate, plus a filtered ``iter`` walk for scale. Run it
against two builds (crates.io uppsala vs the local working tree) to see
whether the upstream XPath AST cache is amortized through the binding.

Usage:

    uv run python benchmarks/xpath_pattern.py [AGGREGATE.xml] [--runs 30]

The aggregate defaults to the pyFF test fixture in a sibling checkout, like
``etree_bench.py``.
"""

from __future__ import annotations

import argparse
import os
import statistics
import time

from pyuppsala import etree as PU

# Trusted 7 MB aggregate: raise the anti-DoS node-visit budget (documented knob).
PU.MAX_XPATH_NODE_VISITS = 10_000_000

DEFAULT_AGGREGATE = os.path.join(
    os.path.dirname(__file__),
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
MD_NS = "urn:oasis:names:tc:SAML:2.0:metadata"


def bench(fn, runs=30, warmup=3):
    for _ in range(warmup):
        fn()
    ts = []
    for _ in range(runs):
        t0 = time.perf_counter()
        fn()
        ts.append((time.perf_counter() - t0) * 1000)
    return statistics.median(ts)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("aggregate", nargs="?", default=DEFAULT_AGGREGATE)
    parser.add_argument("--runs", type=int, default=30)
    args = parser.parse_args(argv)
    if not os.path.isfile(args.aggregate):
        parser.error("aggregate file not found: %s" % args.aggregate)

    with open(args.aggregate, "rb") as fh:
        xml = fh.read()
    root = PU.fromstring(xml)
    print("aggregate: %s (%d bytes)" % (args.aggregate, len(xml)))
    print("root: %s, children: %d" % (root.tag, len(root)))
    runs = args.runs

    expr = "//md:EntityDescriptor/md:IDPSSODescriptor"
    ns = {"md": MD_NS}
    xp = PU.XPath(expr, namespaces=ns)
    compiled = bench(lambda: len(xp(root)), runs)
    oneshot = bench(lambda: len(PU.XPath(expr, namespaces=ns)(root)), runs)
    print("xpath full-doc:  compiled %8.3f ms | oneshot %8.3f ms" % (compiled, oneshot))

    first = root[0]
    xp2 = PU.XPath("string(@entityID)")
    compiled_cheap = bench(lambda: xp2(first), runs) * 1000
    oneshot_cheap = bench(lambda: PU.XPath("string(@entityID)")(first), runs) * 1000
    print("xpath cheap:     compiled %8.1f us | oneshot %8.1f us" % (compiled_cheap, oneshot_cheap))

    desc = bench(lambda: list(root.iter("{%s}EntityDescriptor" % MD_NS)), runs)
    print("iter EntityDescriptor: %8.3f ms" % desc)


if __name__ == "__main__":
    main()
