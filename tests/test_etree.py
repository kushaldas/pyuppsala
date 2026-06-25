"""Tests for pyuppsala.etree (lxml.etree-compatible API).

The bulk are *differential* tests: the same operations run through both
``pyuppsala.etree`` and the real ``lxml.etree``, asserting matching results.
These are skipped if lxml is not installed. A second group of standalone tests
covers pyuppsala-specific behavior (security limits, unsupported parser
options, exception identity) that should not depend on lxml.
"""

import pytest

from pyuppsala import etree as P

# lxml is only needed by the differential tests. Import it optionally so the
# standalone tests (TestStandalone) still run in environments without lxml.
try:
    import lxml.etree as L

    HAS_LXML = True
except ImportError:  # pragma: no cover - depends on the environment
    L = None
    HAS_LXML = False

requires_lxml = pytest.mark.skipif(not HAS_LXML, reason="lxml is not installed")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

SAMPLE = (
    "<root id='r'>"
    "head"
    "<child k='1'>one</child>tail1"
    "<child k='2'>two</child>tail2"
    "<other><deep>D</deep></other>"
    "<!--c-->"
    "</root>"
)

NS_DOC = (
    "<a:root xmlns:a='http://a' xmlns:b='http://b'>"
    "<a:item>x</a:item>"
    "<b:item>y</b:item>"
    "</a:root>"
)


def lxml_canon(xml_text):
    """Reparse a serialized string with lxml and re-serialize, to neutralize
    formatting differences (self-closing tags, etc.) while checking semantics."""
    return L.tostring(L.fromstring(xml_text))


def walk_compare(pe, le):
    """Recursively assert two element trees agree on tag/text/tail/attrib."""
    if not isinstance(pe.tag, str) or not isinstance(le.tag, str):
        # Comment / PI node: compare by callable kind + text content.
        assert callable(pe.tag) and callable(le.tag), (pe.tag, le.tag)
        assert (pe.text or None) == (le.text or None), (pe.text, le.text)
        assert (pe.tail or None) == (le.tail or None), (pe.tail, le.tail)
        return
    assert pe.tag == le.tag, (pe.tag, le.tag)
    assert (pe.text or None) == (le.text or None), (pe.tag, pe.text, le.text)
    assert (pe.tail or None) == (le.tail or None), (pe.tag, pe.tail, le.tail)
    assert dict(pe.attrib) == dict(le.attrib), (pe.tag, dict(pe.attrib), dict(le.attrib))
    pkids = list(pe)
    lkids = list(le)
    assert len(pkids) == len(lkids), (pe.tag, len(pkids), len(lkids))
    for pc, lc in zip(pkids, lkids):
        walk_compare(pc, lc)


# ---------------------------------------------------------------------------
# Differential: parsing & navigation
# ---------------------------------------------------------------------------


@requires_lxml
class TestParseDifferential:
    def test_full_tree_walk(self):
        walk_compare(P.fromstring(SAMPLE), L.fromstring(SAMPLE))

    def test_namespaced_tree_walk(self):
        walk_compare(P.fromstring(NS_DOC), L.fromstring(NS_DOC))

    def test_tag_text_tail(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert pr.tag == lr.tag == "root"
        assert pr.text == lr.text == "head"
        assert pr[0].text == lr[0].text == "one"
        assert pr[0].tail == lr[0].tail == "tail1"

    def test_len_and_index(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert len(pr) == len(lr)
        assert pr.index(pr[1]) == lr.index(lr[1])

    def test_getparent_getnext_getprevious(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert pr[1].getparent().tag == lr[1].getparent().tag
        assert pr[0].getnext().tag == lr[0].getnext().tag
        assert pr[1].getprevious().tag == lr[1].getprevious().tag
        assert pr.getparent() is None and lr.getparent() is None


# ---------------------------------------------------------------------------
# Differential: search
# ---------------------------------------------------------------------------


@requires_lxml
class TestSearchDifferential:
    @pytest.mark.parametrize(
        "path",
        [
            "child",
            "other/deep",
            ".//deep",
            "child[@k='2']",
            "*",
            "child[2]",
            ".//child",
        ],
    )
    def test_findall(self, path):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert [e.tag for e in pr.findall(path)] == [e.tag for e in lr.findall(path)]

    def test_findtext(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert pr.findtext("child") == lr.findtext("child")
        assert pr.findtext("nope", "def") == lr.findtext("nope", "def")

    def test_find_namespaced(self):
        ns = {"a": "http://a", "b": "http://b"}
        pr, lr = P.fromstring(NS_DOC), L.fromstring(NS_DOC)
        assert pr.find("a:item", ns).text == lr.find("a:item", ns).text
        assert [e.tag for e in pr.findall("b:item", ns)] == [
            e.tag for e in lr.findall("b:item", ns)
        ]

    @pytest.mark.parametrize(
        "path",
        ["*", ".//*", "*[1]", "*[2]", "*[last()]", "a[@k='2']", ".//a", "b/a"],
    )
    def test_wildcards_with_comments(self, path):
        # Comments/PIs are children in our tree (like lxml) but must be excluded
        # from `*`/`//*` and never reach positional predicates -- verify the
        # stdlib ElementPath delegation matches lxml on a comment-bearing doc.
        doc = "<r><a k='1'/>t<!--c--><a k='2'/><b><a/></b></r>"

        def names(els):
            return ["#" if not isinstance(e.tag, str) else e.tag for e in els]

        assert names(P.fromstring(doc).findall(path)) == names(
            L.fromstring(doc).findall(path)
        )

    def test_iter(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert [e.tag for e in pr.iter("child")] == [e.tag for e in lr.iter("child")]

    def test_iter_wildcard_vs_none(self):
        # iter("*") is an element-only wildcard; iter() yields comments/PIs too.
        doc = "<r><a/><!--c--><?pi x?><b/></r>"
        pr, lr = P.fromstring(doc), L.fromstring(doc)

        def names(it):
            return ["#" if not isinstance(e.tag, str) else e.tag for e in it]

        assert names(pr.iter("*")) == names(lr.iter("*"))
        assert names(pr.iter()) == names(lr.iter())

    def test_itertext(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert list(pr.itertext()) == list(lr.itertext())

    def test_xpath(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert [e.tag for e in pr.xpath("//child")] == [
            e.tag for e in lr.xpath("//child")
        ]
        assert pr.xpath("count(//child)") == lr.xpath("count(//child)")


# ---------------------------------------------------------------------------
# Differential: attributes
# ---------------------------------------------------------------------------


@requires_lxml
class TestAttribDifferential:
    def test_get_keys_items(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert pr.get("id") == lr.get("id")
        assert pr[0].get("k") == lr[0].get("k")
        assert sorted(pr.keys()) == sorted(lr.keys())
        assert dict(pr.items()) == dict(lr.items())

    def test_set_and_serialize(self):
        pr, lr = P.fromstring("<a/>"), L.fromstring("<a/>")
        pr.set("x", "1")
        lr.set("x", "1")
        assert lxml_canon(P.tostring(pr, encoding="unicode")) == L.tostring(lr)

    def test_attrib_mapping(self):
        pr, lr = P.fromstring(SAMPLE), L.fromstring(SAMPLE)
        assert ("id" in pr.attrib) == ("id" in lr.attrib)
        assert pr.attrib["id"] == lr.attrib["id"]
        with pytest.raises(KeyError):
            pr.attrib["missing"]


# ---------------------------------------------------------------------------
# Differential: building & serialization
# ---------------------------------------------------------------------------


def build_tree(etree_mod):
    root = etree_mod.Element("root", {"v": "1"})
    a = etree_mod.SubElement(root, "a")
    a.text = "hello"
    a.tail = "after"
    b = etree_mod.SubElement(root, "b")
    b.set("flag", "yes")
    return root


@requires_lxml
class TestBuildDifferential:
    def test_build_roundtrip(self):
        pr = build_tree(P)
        lr = build_tree(L)
        assert lxml_canon(P.tostring(pr, encoding="unicode")) == L.tostring(lr)

    def test_namespaced_build(self):
        pr = P.Element("{http://n}top", nsmap={"p": "http://n"})
        P.SubElement(pr, "{http://n}kid").text = "v"
        lr = L.Element("{http://n}top", nsmap={"p": "http://n"})
        L.SubElement(lr, "{http://n}kid").text = "v"
        assert lxml_canon(P.tostring(pr, encoding="unicode")) == L.tostring(lr)

    def test_comment_in_tree(self):
        pr = build_tree(P)
        pr.append(P.Comment("note"))
        lr = build_tree(L)
        lr.append(L.Comment("note"))
        assert lxml_canon(P.tostring(pr, encoding="unicode")) == L.tostring(lr)

    def test_tostring_bytes_default(self):
        pr = build_tree(P)
        assert isinstance(P.tostring(pr), bytes)


# ---------------------------------------------------------------------------
# Differential: mutation
# ---------------------------------------------------------------------------


@requires_lxml
class TestMutationDifferential:
    def test_append_insert_remove(self):
        def run(m):
            r = m.Element("r")
            for name in ("a", "b", "c"):
                m.SubElement(r, name)
            r.insert(1, m.Element("x"))
            r.remove(r.find("b"))
            return [e.tag for e in r]

        assert run(P) == run(L)

    def test_replace(self):
        def run(m):
            r = m.fromstring("<r><a/><b/></r>")
            r.replace(r[0], m.Element("z"))
            return [e.tag for e in r]

        assert run(P) == run(L)

    def test_remove_drops_tail(self):
        def run(m):
            r = m.fromstring("<r><a/>tail<b/></r>")
            r.remove(r[0])
            return m.tostring(r, encoding="unicode")

        assert lxml_canon(run(P)) == lxml_canon(run(L))

    def test_cross_tree_move(self):
        def run(m):
            src = m.fromstring("<src><moved>M</moved>t</src>")
            dst = m.Element("dst")
            dst.append(src[0])
            return (
                m.tostring(dst, encoding="unicode"),
                m.tostring(src, encoding="unicode"),
            )

        pd, ps = run(P)
        ld, ls = run(L)
        assert lxml_canon(pd) == lxml_canon(ld)
        assert lxml_canon(ps) == lxml_canon(ls)

    def test_text_tail_mutation(self):
        def run(m):
            r = m.Element("r")
            a = m.SubElement(r, "a")
            a.text = "T"
            a.tail = "L"
            a.text = None
            return m.tostring(r, encoding="unicode")

        assert lxml_canon(run(P)) == lxml_canon(run(L))


# ---------------------------------------------------------------------------
# Differential: namespaces / QName
# ---------------------------------------------------------------------------


@requires_lxml
class TestNamespaceDifferential:
    def test_nsmap(self):
        pr, lr = P.fromstring(NS_DOC), L.fromstring(NS_DOC)
        assert pr.nsmap == lr.nsmap

    def test_clark_tag(self):
        pr, lr = P.fromstring(NS_DOC), L.fromstring(NS_DOC)
        assert pr[0].tag == lr[0].tag == "{http://a}item"

    def test_qname(self):
        assert str(P.QName("http://n", "x")) == str(L.QName("http://n", "x"))
        assert P.QName("{http://n}x").localname == L.QName("{http://n}x").localname


# ---------------------------------------------------------------------------
# Differential: ElementTree & validation
# ---------------------------------------------------------------------------


@requires_lxml
class TestElementTreeDifferential:
    def test_parse_file(self, tmp_path):
        f = tmp_path / "d.xml"
        f.write_text(SAMPLE)
        pt = P.parse(str(f))
        lt = L.parse(str(f))
        assert pt.getroot().tag == lt.getroot().tag
        assert [e.tag for e in pt.findall("child")] == [
            e.tag for e in lt.findall("child")
        ]

    def test_write(self, tmp_path):
        pr = build_tree(P)
        f = tmp_path / "o.xml"
        P.ElementTree(pr).write(str(f))
        assert lxml_canon(f.read_text()) == L.tostring(build_tree(L))


SCHEMA = (
    "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>"
    "<xs:element name='note' type='xs:string'/>"
    "</xs:schema>"
)


@requires_lxml
class TestSchemaDifferential:
    def test_valid(self):
        schema_p = P.XMLSchema(P.fromstring(SCHEMA))
        schema_l = L.XMLSchema(L.fromstring(SCHEMA))
        doc_p = P.fromstring("<note>hi</note>")
        doc_l = L.fromstring("<note>hi</note>")
        assert schema_p.validate(doc_p) == schema_l.validate(doc_l) is True

    def test_invalid(self):
        schema_p = P.XMLSchema(P.fromstring(SCHEMA))
        bad = P.fromstring("<wrong/>")
        assert schema_p.validate(bad) is False
        with pytest.raises(P.DocumentInvalid):
            schema_p.assertValid(bad)


@requires_lxml
class TestExtendedDifferential:
    def test_slicing(self):
        xml = "<r><a/><b/><c/><d/></r>"
        pr, lr = P.fromstring(xml), L.fromstring(xml)
        assert [e.tag for e in pr[1:3]] == [e.tag for e in lr[1:3]]

    def test_extend(self):
        def run(m):
            r = m.Element("r")
            r.extend([m.Element("x"), m.Element("y")])
            return [e.tag for e in r]

        assert run(P) == run(L)

    def test_del_slice(self):
        def run(m):
            r = m.fromstring("<r><a/><b/><c/></r>")
            del r[0:2]
            return [e.tag for e in r]

        assert run(P) == run(L)

    def test_processing_instruction(self):
        def run(m):
            r = m.Element("r")
            r.append(m.ProcessingInstruction("php", "echo 1"))
            return m.tostring(r, encoding="unicode")

        assert lxml_canon(run(P)) == lxml_canon(run(L))

    def test_addnext_addprevious(self):
        def run(m):
            r = m.fromstring("<r><a/></r>")
            r[0].addnext(m.Element("z"))
            r[0].addprevious(m.Element("y"))
            return [e.tag for e in r]

        assert run(P) == run(L) == ["y", "a", "z"]

    def test_fromstringlist(self):
        assert (
            P.fromstringlist(["<a>", "x", "</a>"]).text
            == L.fromstringlist(["<a>", "x", "</a>"]).text
        )

    def test_indent(self):
        def run(m):
            r = m.fromstring("<r><a><b>x</b></a></r>")
            m.indent(r)
            return m.tostring(r, encoding="unicode")

        assert run(P) == run(L)

    def test_tounicode(self):
        assert P.tounicode(P.fromstring("<a>1</a>")) == L.tostring(
            L.fromstring("<a>1</a>"), encoding="unicode"
        )

    def test_getpath(self):
        def run(m):
            r = m.fromstring("<r><a/><a/><b/></r>")
            return m.ElementTree(r).getpath(r[1])

        assert run(P) == run(L) == "/r/a[2]"


# ---------------------------------------------------------------------------
# Standalone (pyuppsala-specific, no lxml oracle)
# ---------------------------------------------------------------------------


class TestStandalone:
    def test_syntax_error_is_lxml_named(self):
        with pytest.raises(P.XMLSyntaxError):
            P.fromstring("<a></b>")

    def test_parse_error_alias(self):
        assert P.ParseError is P.XMLSyntaxError
        assert issubclass(P.XMLSyntaxError, P.LxmlError)

    def test_unsupported_parser_options_raise(self):
        for kwargs in (
            {"recover": True},
            {"dtd_validation": True},
            {"load_dtd": True},
            {"resolve_entities": False},
        ):
            with pytest.raises(NotImplementedError):
                P.XMLParser(**kwargs)

    def test_cosmetic_parser_options_ignored(self):
        parser = P.XMLParser(collect_ids=False, compact=False, no_network=True)
        root = P.fromstring("<a><b/></a>", parser)
        assert root.tag == "a"

    def test_remove_comments(self):
        parser = P.XMLParser(remove_comments=True)
        root = P.fromstring("<a><!--x--><b/></a>", parser)
        assert [e.tag for e in root] == ["b"]

    def test_huge_tree_allows_deep_nesting(self):
        depth = 500
        xml = "<r>" + "<n>" * depth + "</n>" * depth + "</r>"
        with pytest.raises(P.XMLSyntaxError):
            P.fromstring(xml)  # default depth limit blocks it
        parser = P.XMLParser(huge_tree=True)
        root = P.fromstring(xml, parser)
        assert root.tag == "r"

    def test_identity_stability(self):
        root = P.fromstring(SAMPLE)
        assert root[0] is root[0]
        assert root.find("child") is root[0]

    def test_iselement(self):
        root = P.fromstring("<a/>")
        assert P.iselement(root)
        assert not P.iselement("a")

    def test_fromstringlist_accepts_generator(self):
        # A generator (not a sequence) must work, not just lists.
        gen = (part for part in ("<a>", "x", "</a>"))
        root = P.fromstringlist(gen)
        assert root.tag == "a"
        assert root.text == "x"

    def test_parse_bom_prefixed_bytes(self):
        # UTF-8 BOM-prefixed bytes are XML content, not a filename.
        data = b"\xef\xbb\xbf<doc>hi</doc>"
        tree = P.parse(data)
        assert tree.getroot().tag == "doc"

    @pytest.mark.parametrize("encoding", ["utf-16-le", "utf-16-be"])
    def test_parse_utf16_without_bom(self, encoding):
        # UTF-16 (LE/BE) XML without a BOM must be recognized as content, not a
        # filename, matching the native parse_bytes decoder.
        raw = "<doc>hi</doc>".encode(encoding)
        root = P.parse(raw).getroot()
        assert root.tag == "doc"
        assert root.text == "hi"

    def test_tostring_rejects_non_xml_method(self):
        el = P.fromstring("<a>x</a>")
        assert P.tostring(el, method="xml", encoding="unicode") == "<a>x</a>"
        for method in ("html", "text", "c14n"):
            with pytest.raises(NotImplementedError):
                P.tostring(el, method=method)

    def test_namespaced_attribute_delete_is_exact(self):
        # Two attributes share a local name in different namespaces; deleting one
        # via Clark notation must not remove the other.
        el = P.Element("e", nsmap={"a": "http://a", "b": "http://b"})
        el.set("{http://a}k", "1")
        el.set("{http://b}k", "2")
        del el.attrib["{http://a}k"]
        assert el.get("{http://a}k") is None
        assert el.get("{http://b}k") == "2"

    def test_plain_attribute_ops_are_no_namespace(self):
        # A plain key refers to the no-namespace attribute, never a namespaced
        # attribute that merely shares the local name.
        el = P.Element("e", nsmap={"a": "http://a"})
        el.set("k", "plain")
        el.set("{http://a}k", "ns")
        assert el.get("k") == "plain"
        assert el.get("{http://a}k") == "ns"
        del el.attrib["k"]  # removes only the no-namespace one
        assert el.get("k") is None
        assert el.keys() == ["{http://a}k"]

    def test_cross_tree_membership_is_rejected(self):
        # node_id is per-document; a node from another tree whose id collides
        # must not be treated as a child by `in`, index(), remove(), replace().
        t1 = P.Element("r")
        P.SubElement(t1, "c")
        t2 = P.Element("x")
        y = P.SubElement(t2, "y")  # y.parent.node_id == r.node_id (both roots)

        assert y not in t1
        with pytest.raises(ValueError):
            t1.index(y)
        with pytest.raises(ValueError):
            t1.remove(y)
        with pytest.raises(ValueError):
            t1.replace(y, P.Element("z"))
        # t1 is untouched and t2 still owns y.
        assert [e.tag for e in t1] == ["c"]
        assert y.getparent().tag == "x"

    def test_index_non_element_raises_valueerror(self):
        root = P.fromstring("<r><a/></r>")
        with pytest.raises(ValueError):
            root.index("not-an-element")

    def test_xpath_variables_raise(self):
        root = P.fromstring("<a><b/></a>")
        with pytest.raises(NotImplementedError):
            root.xpath("//b", x=1)
        with pytest.raises(NotImplementedError):
            P.XPath("//b")(root, x=1)
        with pytest.raises(NotImplementedError):
            P.XPathEvaluator(root)("//b", x=1)

    def test_parser_encoding_override(self):
        # encoding= overrides the declared encoding for byte input.
        raw = '<?xml version="1.0"?><x>caf\u00e9</x>'.encode("latin-1")
        root = P.fromstring(raw, P.XMLParser(encoding="latin-1"))
        assert root.text == "caf\u00e9"
        # An unknown codec surfaces as XMLSyntaxError, not a silent miss.
        with pytest.raises(P.XMLSyntaxError):
            P.fromstring(b"<x/>", P.XMLParser(encoding="not-a-real-codec"))

    def test_iter_star_excludes_comments(self):
        root = P.fromstring("<r><a/><!--c--><b/></r>")
        assert [e.tag for e in root.iter("*")] == ["r", "a", "b"]
        assert [
            "#" if not isinstance(e.tag, str) else e.tag for e in root.iter()
        ] == ["r", "a", "#", "b"]
