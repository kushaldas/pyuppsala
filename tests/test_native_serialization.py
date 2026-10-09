"""Native serialization and XPath fast paths behind ``etree.tostring`` and
``etree.XPath`` (PERFORMANCE.md, 2026-10-08 section).

``tostring`` now serializes straight to ``bytes`` in Rust for the ASCII and
UTF-8 encodings, injects inherited namespace declarations natively, and
decides the default-namespace copy with a native subtree walk; ``XPath``
objects keep one native evaluator. These tests pin the observable contract
against lxml, which the lxml-differential suite in ``test_etree.py`` does not
exercise byte for byte for these paths.
"""

import pyuppsala
from pyuppsala import etree as P

try:
    import lxml.etree as L
except ImportError:  # pragma: no cover
    L = None

import pytest

NS_DOC = (
    '<md:EntitiesDescriptor xmlns:md="urn:md" xmlns:ds="urn:ds" xmlns="urn:dflt">'
    '<md:EntityDescriptor entityID="a&amp;b"><ds:Signature/><plain/>'
    "<md:Text>café €</md:Text></md:EntityDescriptor>"
    "</md:EntitiesDescriptor>"
)


def _both(xml):
    return P.fromstring(xml), (L.fromstring(xml) if L is not None else None)


def test_sub_element_bytes_carry_inherited_declarations_like_lxml():
    root, lroot = _both(NS_DOC)
    ours = P.tostring(root[0])
    assert ours.startswith(b"<md:EntityDescriptor ")
    assert b'xmlns:md="urn:md"' in ours
    assert b'xmlns:ds="urn:ds"' in ours
    assert b'xmlns="urn:dflt"' in ours
    # The fragment re-parses on its own and keeps its expanded names.
    again = P.fromstring(ours)
    assert again.tag == "{urn:md}EntityDescriptor"
    assert again[0].tag == "{urn:ds}Signature"
    assert again[1].tag == "{urn:dflt}plain"
    if L is not None:
        assert P.fromstring(L.tostring(lroot[0])).tag == again.tag


def test_default_ascii_output_uses_decimal_character_references():
    root, lroot = _both(NS_DOC)
    ours = P.tostring(root)
    assert isinstance(ours, bytes)
    assert b"caf&#233; &#8364;" in ours
    assert ours.isascii()
    if L is not None:
        assert ours == L.tostring(lroot)


def test_utf8_output_is_raw_utf8_with_declaration_rules():
    root, lroot = _both(NS_DOC)
    ours = P.tostring(root, encoding="utf-8")
    assert "café €".encode("utf-8") in ours
    assert not ours.startswith(b"<?xml")
    if L is not None:
        assert ours == L.tostring(lroot, encoding="utf-8")
    declared = P.tostring(root, encoding="utf-8", xml_declaration=True)
    assert declared.startswith(b'<?xml version="1.0" encoding="utf-8"?>\n')
    if L is not None:
        # lxml quotes the declaration with single quotes; the body is identical.
        theirs = L.tostring(lroot, encoding="utf-8", xml_declaration=True)
        assert declared.split(b"\n", 1)[1] == theirs.split(b"\n", 1)[1]


def test_unicode_and_other_codecs_still_go_through_str():
    root, lroot = _both(NS_DOC)
    text = P.tostring(root, encoding="unicode")
    assert isinstance(text, str) and "café €" in text
    latin = P.tostring(root, encoding="iso-8859-1")
    assert latin.startswith(b'<?xml version="1.0" encoding="iso-8859-1"?>')
    assert b"caf\xe9 &#8364;" in latin
    if L is not None:
        theirs = L.tostring(lroot, encoding="iso-8859-1")
        assert latin.split(b"\n", 1)[1] == theirs.split(b"\n", 1)[1]


def test_pretty_print_bytes_end_with_newline_and_match_str_form():
    root, _ = _both("<r><a><b/></a></r>")
    as_bytes = P.tostring(root, pretty_print=True)
    as_text = P.tostring(root, pretty_print=True, encoding="unicode")
    assert as_bytes.endswith(b"\n")
    assert as_bytes == as_text.encode("ascii")


def test_doctype_prefix_is_bytes_on_the_bytes_path():
    tree = P.ElementTree(P.fromstring("<r/>"))
    out = P.tostring(tree, doctype="<!DOCTYPE r>")
    assert out == b"<!DOCTYPE r>\n<r/>"
    assert P.tostring(tree, doctype="<!DOCTYPE r>", encoding="unicode") == "<!DOCTYPE r>\n<r/>"


def test_native_bare_name_check_matches_python_walk_semantics():
    # A bare child under an explicit default namespace needs the qualified
    # copy; an xmlns="" reset below it stops the qualification.
    root = P.fromstring('<r xmlns="urn:a"><x/><y xmlns=""><z/></y></r>')
    node = root._node
    assert node.has_bare_name_under_default_namespace(None) is False
    # Built tree: a bare-named element whose ancestor declares a default.
    parent = P.Element("{urn:p}parent", nsmap={None: "urn:p"})
    child = P.SubElement(parent, "bare")
    assert child.tag == "bare"
    assert parent._node.has_bare_name_under_default_namespace(None) is True
    assert child._node.has_bare_name_under_default_namespace("urn:p") is True
    assert child._node.has_bare_name_under_default_namespace(None) is False
    # tostring still produces lxml's lexical form.
    out = P.tostring(parent)
    if L is not None:
        lparent = L.Element("{urn:p}parent", nsmap={None: "urn:p"})
        L.SubElement(lparent, "bare")
        assert out == L.tostring(lparent)


def test_xpath_object_reuses_evaluator_and_tracks_budget(monkeypatch):
    root = P.fromstring("<r>" + "<a/>" * 50 + "</r>")
    xp = P.XPath("count(//a)")
    assert xp(root) == 50.0
    first = xp._evaluator
    assert xp(root) == 50.0 and xp._evaluator is first
    # Raising or lowering the module budget after construction takes effect.
    monkeypatch.setattr(P, "MAX_XPATH_NODE_VISITS", 10)
    with pytest.raises(P.XPathEvalError):
        xp(root)
    assert xp._evaluator is not first
    monkeypatch.setattr(P, "MAX_XPATH_NODE_VISITS", pyuppsala.DEFAULT_MAX_XPATH_NODE_VISITS)
    assert xp(root) == 50.0
    with pytest.raises(NotImplementedError):
        xp(root, v=1)
    tree = P.ElementTree(root)
    assert P.XPath("count(//a)")(tree) == 50.0


def test_fused_descendant_steps_agree_with_lxml():
    xml = (
        '<r xmlns:m="urn:m"><a id="1"><b id="2"/><a id="3"><b id="4"/><m:b id="5"/></a></a>'
        '<b id="6"><c/></b><m:b id="7"/></r>'
    )
    root, lroot = _both(xml)
    ns = {"m": "urn:m"}
    for expr in [
        "//b", "//m:b", "//m:*", "//*", "//@id", "//a//b", "//a//@id", "//b[1]",
        "//b[last()]", "//b[@id='4']", "count(//*)", "count(//@id)", ".//b", "//c//*",
    ]:
        ours = root.xpath(expr, namespaces=ns)
        if isinstance(ours, list):
            ours = [e.get("id") if hasattr(e, "get") else e for e in ours]
        if L is not None:
            theirs = lroot.xpath(expr, namespaces=ns)
            if isinstance(theirs, list):
                theirs = [e.get("id") if hasattr(e, "get") else e for e in theirs]
            assert ours == theirs, expr
    assert root.xpath("//zz:b", namespaces=ns) == []
