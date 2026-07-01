"""Side-by-side micro-benchmark of ``pyuppsala.etree`` against ``lxml.etree``.

The operations here are shaped after how pyFF actually drives an etree backend
when it loads, selects, tidies and signs a SAML metadata aggregate. The goal is
NOT to time arbitrary API calls but to reproduce pyFF's hot patterns so that the
per-operation ``pyuppsala / lxml`` ratio tells us exactly which native primitive
to optimise next (see pyFF/performance.md for the end-to-end macro numbers and
the root-cause analysis that motivated this harness).

Each pyFF call site is cited on the operation that models it. The benchmark
parses one large EntitiesDescriptor aggregate (default:
pyFF/src/pyff/test/data/metadata/swamid-2.0-test.xml, ~1000 entities) once per
backend, then runs each operation under both backends, auto-scaling the
repetition count so every measurement runs for a similar wall-clock budget.

Usage:

    python benchmarks/etree_bench.py [AGGREGATE.xml] [--budget 0.5] [--json out.json]

lxml is an optional, dev-only dependency: if it is not importable the benchmark
still runs (pyuppsala-only) and simply omits the ratio column.
"""

from __future__ import annotations

import argparse
import gc
import json
import os
import sys
import time
from dataclasses import dataclass, field
from typing import Callable, Optional

# The pyuppsala backend is mandatory; lxml is optional so the harness can run on
# hosts where only pyuppsala is installed (the ratio column is omitted there).
from pyuppsala import etree as PU

try:
    from lxml import etree as LX  # type: ignore
except Exception:  # pragma: no cover - lxml is a dev convenience only
    LX = None


# SAML / metadata namespaces used by the entity-shaped operations below. These
# mirror the constants pyFF keeps in pyff.constants.NS.
NS = {
    "md": "urn:oasis:names:tc:SAML:2.0:metadata",
    "mdattr": "urn:oasis:names:tc:SAML:metadata:attribute",
    "saml": "urn:oasis:names:tc:SAML:2.0:assertion",
    "ds": "http://www.w3.org/2000/09/xmldsig#",
}
ENTITY_TAG = "{%s}EntityDescriptor" % NS["md"]
IDP_TAG = "{%s}IDPSSODescriptor" % NS["md"]
SP_TAG = "{%s}SPSSODescriptor" % NS["md"]
EA_TAG = "{%s}EntityAttributes" % NS["mdattr"]
ATTR_TAG = "{%s}Attribute" % NS["saml"]
ATTRVAL_TAG = "{%s}AttributeValue" % NS["saml"]

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


@dataclass
class Backend:
    """One etree implementation under test (its module plus a parsed tree).

    ``root`` is the parsed aggregate root element, ``entities`` is the list of
    its ``EntityDescriptor`` children, and ``entity_bytes`` is the per-entity
    serialized form used by parse-each-entity operations (computed once, with
    lxml, so both backends parse byte-identical inputs).
    """

    name: str
    et: object
    root: object = None
    entities: list = field(default_factory=list)


@dataclass
class Result:
    """Timing for one operation under one backend (seconds per call)."""

    seconds: Optional[float]
    calls: int
    note: str = ""


def _time(fn: Callable[[], object], budget: float) -> Result:
    """Run ``fn`` repeatedly for about ``budget`` seconds; return seconds/call.

    The function is warmed once (to populate any caches and to surface errors),
    then the repetition count is chosen from a single timed rep so that the
    measured batch runs for roughly ``budget`` seconds. GC is disabled during
    measurement to reduce noise. A raised exception is captured as a note rather
    than aborting the whole benchmark.
    """
    try:
        fn()  # warm-up (also catches unsupported operations early)
    except Exception as e:  # pragma: no cover - reported, not fatal
        return Result(None, 0, note="%s: %s" % (type(e).__name__, e))

    # Estimate cost from one rep, then size the batch to the time budget.
    t0 = time.perf_counter()
    fn()
    one = time.perf_counter() - t0
    reps = 1 if one <= 0 else max(1, int(budget / one))

    gc_was_enabled = gc.isenabled()
    gc.disable()
    try:
        t0 = time.perf_counter()
        for _ in range(reps):
            fn()
        elapsed = time.perf_counter() - t0
    finally:
        if gc_was_enabled:
            gc.enable()
    return Result(elapsed / reps, reps)


# ---------------------------------------------------------------------------
# Operations. Each takes a Backend and returns a zero-arg callable that performs
# the pyFF-shaped work once. Keeping the closure creation out of the timed path
# means we measure the operation, not the setup.
# ---------------------------------------------------------------------------

def op_parse_aggregate(b: Backend, blob: bytes):
    """Parse the whole 7 MB aggregate (pyFF parse_xml, utils.py / samlmd.py)."""
    et = b.et
    return lambda: et.fromstring(blob)


def op_parse_entities(b: Backend, entity_blobs: list):
    """Parse each entity document separately.

    Models pyFF loading a directory of per-entity metadata files
    (resource.py / samlmd.parse_saml_metadata) rather than one aggregate.
    """
    et = b.et

    def run():
        for blob in entity_blobs:
            et.fromstring(blob)

    return run


def op_iter_entitydescriptor(b: Backend):
    """Full-tree filtered walk: root.iter(EntityDescriptor).

    Models select / store.update enumerating every entity in the aggregate.
    """
    root = b.root
    return lambda: [e for e in root.iter(ENTITY_TAG)]


def op_has_tag_per_entity(b: Backend):
    """Find-first per entity: next(e.iter(tag)) for IDP and SP roles.

    Models pyFF is_idp / is_sp / has_tag (samlmd.py:601-609, utils.py:664),
    called for every entity during select and indexing.
    """
    entities = b.entities
    sentinel = object()

    def run():
        n = 0
        for e in entities:
            if next(e.iter(IDP_TAG), sentinel) is not sentinel:
                n += 1
            if next(e.iter(SP_TAG), sentinel) is not sentinel:
                n += 1
        return n

    return run


def op_with_entity_attributes(b: Backend):
    """Nested iter over EntityAttributes/Attribute/AttributeValue per entity.

    Models pyFF with_entity_attributes (samlmd.py:621), used by entity-attribute
    indexing and discovery filters.
    """
    entities = b.entities

    def run():
        out = 0
        for entity in entities:
            for ea in entity.iter(EA_TAG):
                for a in ea.iter(ATTR_TAG):
                    for v in a.iter(ATTRVAL_TAG):
                        if v.text is not None:
                            out += 1
        return out

    return run


def op_with_tree(b: Backend):
    """Recursive whole-tree visit via list(elt) (pyFF with_tree, utils.py:484).

    Models check_xml_namespaces / _verify walking every element once.
    """
    root = b.root

    def walk(elt):
        # pyFF's with_tree recurses with `for child in list(elt)`.
        if isinstance(elt.tag, str):
            for child in list(elt):
                walk(child)

    return lambda: walk(root)


def op_findall_predicate(b: Backend, sample_id: str):
    """ElementPath findall with an attribute predicate.

    Models pyFF lookups like find EntityDescriptor[@entityID='...'].
    """
    root = b.root
    path = ENTITY_TAG + "[@entityID='%s']" % sample_id
    return lambda: root.findall(path)


def op_xpath_ns(b: Backend):
    """Namespaced XPath returning an attribute node-set.

    Models pyFF's many .xpath(..., namespaces=NS) call sites (samlmd.py,
    builtins.py). Selecting @entityID exercises the attribute axis.
    """
    root = b.root
    return lambda: root.xpath("//md:EntityDescriptor/@entityID", namespaces=NS)


def op_tostring_whole(b: Backend):
    """Serialize the whole aggregate (pyFF publish / pre-sign serialization)."""
    et = b.et
    root = b.root
    return lambda: et.tostring(root)


def op_tostring_per_entity(b: Backend):
    """Serialize each entity subtree separately.

    Models per-entity serialization (MDQ responses, fragment signing); this is
    where inherited-namespace handling matters.
    """
    et = b.et
    entities = b.entities

    def run():
        for e in entities:
            et.tostring(e)

    return run


def op_build_aggregate(b: Backend):
    """Build a fresh EntitiesDescriptor by deepcopy(entity) + append per entity.

    Models pyFF entitiesdescriptor() (samlmd.py:440), the aggregation step. We
    deepcopy then append exactly as pyFF does, so both backends do equivalent
    work (lxml append moves nodes, pyuppsala append copies; the explicit
    deepcopy makes the comparison apples-to-apples).
    """
    et = b.et
    entities = b.entities
    import copy

    def run():
        agg = et.Element(ENTITY_TAG.replace("EntityDescriptor", "EntitiesDescriptor"))
        for e in entities:
            agg.append(copy.deepcopy(e))
        return agg

    return run


def op_nsmap_per_entity(b: Backend):
    """Access .nsmap for every entity (namespace scope walking)."""
    entities = b.entities
    return lambda: [e.nsmap for e in entities]


def op_attr_get(b: Backend):
    """Read .get('entityID') for every entity (attribute access)."""
    entities = b.entities
    return lambda: [e.get("entityID") for e in entities]


# Registry of (label, factory) pairs. Factories that need extra inputs are
# wrapped at build time in main().
def build_ops(blob: bytes, entity_blobs: list, sample_id: str):
    """Return the ordered list of (label, op-factory) the benchmark runs."""
    return [
        ("parse_aggregate", lambda b: op_parse_aggregate(b, blob)),
        ("parse_entities", lambda b: op_parse_entities(b, entity_blobs)),
        ("iter_entitydescriptor", op_iter_entitydescriptor),
        ("has_tag_per_entity", op_has_tag_per_entity),
        ("with_entity_attributes", op_with_entity_attributes),
        ("with_tree", op_with_tree),
        ("findall_predicate", lambda b: op_findall_predicate(b, sample_id)),
        ("xpath_ns", op_xpath_ns),
        ("tostring_whole", op_tostring_whole),
        ("tostring_per_entity", op_tostring_per_entity),
        ("build_aggregate", op_build_aggregate),
        ("nsmap_per_entity", op_nsmap_per_entity),
        ("attr_get", op_attr_get),
    ]


def make_backend(name: str, et, blob: bytes) -> Backend:
    """Parse the aggregate with ``et`` and collect its EntityDescriptors."""
    root = et.fromstring(blob)
    entities = list(root.iter(ENTITY_TAG))
    return Backend(name=name, et=et, root=root, entities=entities)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "aggregate",
        nargs="?",
        default=DEFAULT_AGGREGATE,
        help="Path to a SAML EntitiesDescriptor aggregate XML file.",
    )
    parser.add_argument(
        "--budget",
        type=float,
        default=0.5,
        help="Approx wall-clock seconds to spend timing each operation.",
    )
    parser.add_argument(
        "--json",
        default=None,
        help="Optional path to write the results as JSON.",
    )
    parser.add_argument(
        "--max-entities",
        type=int,
        default=0,
        help="Cap the entity count (0 = all). Useful for quick smoke runs.",
    )
    args = parser.parse_args(argv)

    # DEFAULT_AGGREGATE points at a sibling pyFF checkout that this repository
    # does not ship, so fail with a clear message (rather than a bare
    # FileNotFoundError) when the aggregate is missing.
    if not os.path.isfile(args.aggregate):
        parser.error(
            "aggregate file not found: %s\n"
            "Pass the path to a SAML metadata aggregate as the positional "
            "argument." % args.aggregate
        )

    with open(args.aggregate, "rb") as fh:
        blob = fh.read()
    print("aggregate: %s (%d bytes)" % (args.aggregate, len(blob)))

    # Build per-entity byte blobs once (with lxml when available, else
    # pyuppsala) so every backend's parse_entities op sees identical inputs.
    ref_et = LX if LX is not None else PU
    ref_root = ref_et.fromstring(blob)
    ref_entities = list(ref_root.iter(ENTITY_TAG))
    if args.max_entities and len(ref_entities) > args.max_entities:
        ref_entities = ref_entities[: args.max_entities]
    entity_blobs = [ref_et.tostring(e) for e in ref_entities]
    sample_id = ref_entities[0].get("entityID") if ref_entities else ""
    print("entities: %d, total nodes: %d"
          % (len(ref_entities), sum(1 for _ in ref_root.iter())))

    backends = [Backend(name="pyuppsala", et=PU)]
    if LX is not None:
        backends.append(Backend(name="lxml", et=LX))
    for b in backends:
        b.root = b.et.fromstring(blob)
        b.entities = list(b.root.iter(ENTITY_TAG))
        if args.max_entities:
            b.entities = b.entities[: args.max_entities]

    ops = build_ops(blob, entity_blobs, sample_id)

    # Run every op under every backend.
    rows = []
    for label, factory in ops:
        per_backend = {}
        for b in backends:
            per_backend[b.name] = _time(factory(b), args.budget)
        rows.append((label, per_backend))

    # Pretty table: op | lxml ms | pyuppsala ms | ratio (pu/lxml).
    have_lxml = LX is not None
    header = "%-26s %12s" % ("operation", "pyuppsala")
    if have_lxml:
        header += " %12s %8s" % ("lxml", "ratio")
    print()
    print(header)
    print("-" * len(header))

    out_json = {"aggregate": args.aggregate, "bytes": len(blob), "ops": {}}
    for label, per_backend in rows:
        pu = per_backend.get("pyuppsala")
        line = "%-26s %12s" % (label, _fmt_ms(pu))
        entry = {"pyuppsala_s": pu.seconds if pu else None}
        if have_lxml:
            lx = per_backend.get("lxml")
            ratio = "-"
            if pu and lx and pu.seconds and lx.seconds:
                ratio = "%.2fx" % (pu.seconds / lx.seconds)
            line += " %12s %8s" % (_fmt_ms(lx), ratio)
            entry["lxml_s"] = lx.seconds if lx else None
            entry["ratio"] = (pu.seconds / lx.seconds) if (pu and lx and pu.seconds and lx.seconds) else None
        note = (pu.note if pu and pu.note else "")
        if note:
            line += "   # " + note
        print(line)
        out_json["ops"][label] = entry

    if args.json:
        with open(args.json, "w") as fh:
            json.dump(out_json, fh, indent=2)
        print("\nwrote %s" % args.json)


def _fmt_ms(r: Optional[Result]) -> str:
    """Format a Result as milliseconds, or ERR/n-a for missing/failed runs."""
    if r is None:
        return "n/a"
    if r.seconds is None:
        return "ERR"
    return "%.3f ms" % (r.seconds * 1000.0)


if __name__ == "__main__":
    main()
