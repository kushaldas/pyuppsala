"""Stage-by-stage RSS attribution for the pyuppsala document lifecycle.

Parses one large XML file (e.g. a SAML metadata aggregate) and reports the
process RSS after each lifecycle stage, so memory work can be attributed to a
specific mechanism (parse arena, XPath preparation, serialization buffer,
XSLT, re-parse) instead of a single opaque peak number:

    read bytes -> decode -> parse -> prepare_xpath -> xpath -> tostring
    -> XSLT transform -> re-parse of the transform result

Usage::

    MALLOC_ARENA_MAX=1 python benchmarks/memstages.py AGGREGATE.xml
    MALLOC_ARENA_MAX=1 python benchmarks/memstages.py AGGREGATE.xml --backend lxml

Methodology notes:

- ``MALLOC_ARENA_MAX=1`` avoids glibc scattering allocations over per-thread
  arenas, which makes RSS deltas noisy; the script re-execs itself with it set
  if missing.
- ``malloc_trim(0)`` is called between stages so freed heap is actually
  returned to the kernel and a stage's RSS delta reflects what it *retains*,
  not what glibc happens to cache.
- VmRSS is the current resident size (what this stage retains); VmHWM is the
  high-water mark (transient peaks inside a stage, e.g. ``into_static``'s
  double-arena window, show up here and never go back down).
"""

import argparse
import ctypes
import gc
import os
import sys

MD_NS = "urn:oasis:names:tc:SAML:2.0:metadata"

# An identity transform that drops ds:Signature -- the same shape as pyFF's
# tidy.xsl, so the XSLT stage exercises a realistic whole-document transform.
TIDY_XSLT = (
    '<xsl:stylesheet version="1.0"'
    ' xmlns:xsl="http://www.w3.org/1999/XSL/Transform"'
    ' xmlns:ds="http://www.w3.org/2000/09/xmldsig#">'
    '<xsl:output method="xml" omit-xml-declaration="yes"/>'
    '<xsl:template match="@*|node()">'
    '<xsl:copy><xsl:apply-templates select="@*|node()"/></xsl:copy>'
    "</xsl:template>"
    '<xsl:template match="ds:Signature"/>'
    "</xsl:stylesheet>"
)


def _reexec_with_single_arena():
    """Re-exec under MALLOC_ARENA_MAX=1 so RSS deltas are attributable."""
    if os.environ.get("MALLOC_ARENA_MAX") != "1":
        os.environ["MALLOC_ARENA_MAX"] = "1"
        os.execv(sys.executable, [sys.executable] + sys.argv)


def _malloc_trim():
    """Return freed heap pages to the kernel (glibc only; no-op elsewhere)."""
    try:
        ctypes.CDLL("libc.so.6").malloc_trim(0)
    except OSError:
        pass


def _vm():
    """Return (VmRSS, VmHWM) in MiB from /proc/self/status."""
    rss = hwm = 0
    with open("/proc/self/status") as fd:
        for line in fd:
            if line.startswith("VmRSS:"):
                rss = int(line.split()[1]) // 1024
            elif line.startswith("VmHWM:"):
                hwm = int(line.split()[1]) // 1024
    return rss, hwm


class Reporter:
    """Prints one aligned row per stage: RSS, delta vs previous stage, HWM."""

    def __init__(self):
        self.prev = 0
        print(f"{'stage':<28} {'VmRSS MiB':>10} {'delta':>8} {'VmHWM MiB':>10}")

    def stage(self, name):
        gc.collect()
        _malloc_trim()
        rss, hwm = _vm()
        print(f"{name:<28} {rss:>10} {rss - self.prev:>+8} {hwm:>10}")
        self.prev = rss


def run_pyuppsala(path, rep):
    from pyuppsala import etree as ET

    # The default XPath node-visit budget is an anti-DoS cap for untrusted
    # input; this benchmark evaluates over a known local aggregate that can
    # exceed a million nodes, so raise it (same approach as pyFF).
    ET.MAX_XPATH_NODE_VISITS = max(ET.MAX_XPATH_NODE_VISITS, 50_000_000)

    raw = open(path, "rb").read()
    rep.stage("read bytes (%.1f MB)" % (len(raw) / 1e6))

    text = raw.decode("utf-8")
    rep.stage("decode to str")

    root = ET.fromstring(text)
    del text, raw
    rep.stage("parse (fromstring)")

    doc = ET.native_document(root)
    doc.prepare_xpath()
    rep.stage("prepare_xpath")

    hits = root.xpath("//md:EntityDescriptor", namespaces={"md": MD_NS})
    rep.stage("xpath //EntityDescriptor (%d)" % len(hits))
    del hits

    out = ET.tostring(root, encoding="unicode")
    rep.stage("tostring (%.1f MB)" % (len(out) / 1e6))
    del out
    rep.stage("drop tostring buffer")

    transform = ET.XSLT(ET.fromstring(TIDY_XSLT))
    result = transform(root)
    rep.stage("XSLT transform")

    result_root = result.getroot()
    rep.stage("re-parse XSLT result")
    del result_root, result

    del root, doc
    rep.stage("drop document")


def run_lxml(path, rep):
    from lxml import etree as ET

    raw = open(path, "rb").read()
    rep.stage("read bytes (%.1f MB)" % (len(raw) / 1e6))

    text = raw.decode("utf-8")
    rep.stage("decode to str")

    root = ET.fromstring(text.encode("utf-8"))
    del text, raw
    rep.stage("parse (fromstring)")

    rep.stage("prepare_xpath (n/a)")

    hits = root.xpath("//md:EntityDescriptor", namespaces={"md": MD_NS})
    rep.stage("xpath //EntityDescriptor (%d)" % len(hits))
    del hits

    out = ET.tostring(root, encoding="unicode")
    rep.stage("tostring (%.1f MB)" % (len(out) / 1e6))
    del out
    rep.stage("drop tostring buffer")

    transform = ET.XSLT(ET.fromstring(TIDY_XSLT.encode()))
    result = transform(root)
    rep.stage("XSLT transform")

    result_root = result.getroot()
    rep.stage("re-parse XSLT result (n/a)")
    del result_root, result

    del root
    rep.stage("drop document")


def main():
    _reexec_with_single_arena()
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("xml", help="path to a large XML document")
    ap.add_argument(
        "--backend",
        choices=("pyuppsala", "lxml"),
        default="pyuppsala",
        help="which etree implementation to attribute (default: pyuppsala)",
    )
    args = ap.parse_args()

    print(f"backend: {args.backend}   file: {args.xml}")
    rep = Reporter()
    rep.stage("startup")
    if args.backend == "pyuppsala":
        run_pyuppsala(args.xml, rep)
    else:
        run_lxml(args.xml, rep)


if __name__ == "__main__":
    main()
