#!/usr/bin/env python
"""Fuzz XPath 1.0 evaluation over a fuzzed expression + fuzzed document.

CVE families:
  * J (output/query injection) -- a malformed or hostile XPath expression must
    fail with a clean ``XPathError``, never a panic or a native fault.
  * C (algorithmic DoS) -- deeply nested predicates, huge ``//`` unions and
    positional churn are bounded by the evaluator's depth / node-visit caps;
    the harness lets ``-timeout`` observe any unbounded case.

Input layout mirrors uppsala's ``fuzz_xpath.rs``: the first line is the XPath
expression, the remainder is the XML document (defaulting to ``<r/>``).

Oracle: ``XPathError`` (native or facade) and expression ``SyntaxError`` are the
documented ways an XPath can fail; anything else is a finding.
"""

import sys

import atheris

with atheris.instrument_imports():
    import harness_common as hc


@atheris.instrument_func
def TestOneInput(data: bytes):
    head, tail = hc.split_two(data, b"\n")
    expr = hc.as_text(head)
    xml = hc.as_text(tail) or "<r/>"
    if expr is None:
        return

    root = hc.guard(lambda: hc.ET.fromstring(xml))
    if root is None:
        return

    # etree facade -> native XPathEvaluator. SyntaxError can come from an
    # expression the compiler rejects at a layer that raises it.
    hc.guard(lambda: root.xpath(expr), SyntaxError)


def main():
    atheris.Setup(sys.argv, TestOneInput)
    atheris.Fuzz()


if __name__ == "__main__":
    main()
