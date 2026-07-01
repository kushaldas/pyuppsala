"""Tests for pyuppsala.etree (lxml.etree-compatible API).

The bulk are *differential* tests: the same operations run through both
``pyuppsala.etree`` and the real ``lxml.etree``, asserting matching results.
These are skipped if lxml is not installed. A second group of standalone tests
covers pyuppsala-specific behavior (security limits, unsupported parser
options, exception identity) that should not depend on lxml.
"""

import sys

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
        [
            "*",
            ".//*",
            "*[1]",
            "*[2]",
            "*[last()]",
            "a[@k='2']",
            ".//a",
            "b/a",
            # Namespaced descendant wildcards go through the _is_wildcard_tag
            # branch and must also exclude comments/PIs.
            ".//{*}*",
            ".//{*}*[1]",
            ".//{*}a",
        ],
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

    def test_elementtree_uses_selected_root(self):
        # ElementTree(child) is a view rooted at that child, not at the outer
        # document element.  This matters for serialization and validation
        # because sibling data must not leak into operations on the subtree.
        pr = P.fromstring("<r><a><x/></a><b>secret</b></r>")
        lr = L.fromstring("<r><a><x/></a><b>secret</b></r>")

        pt = P.ElementTree(pr[0])
        lt = L.ElementTree(lr[0])

        assert pt.getroot() is pr[0]
        assert lt.getroot() is lr[0]
        assert pt.getroot().tag == lt.getroot().tag == "a"
        assert lxml_canon(P.tostring(pt, encoding="unicode")) == L.tostring(lt)
        assert pt.getpath(pr[0]) == lt.getpath(lr[0]) == "/r/a"
        assert pt.getpath(pr[0][0]) == lt.getpath(lr[0][0]) == "/a/x"


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


class TestSchemaStandalone:
    """XMLSchema behaviors that do not require a reference lxml install."""

    ANYURI_SCHEMA = (
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>"
        "<xs:element name='u' type='xs:anyURI'/>"
        "</xs:schema>"
    )

    def test_lenient_accepts_anyuri_with_space(self):
        # anyURI with a space is invalid under strict XSD but accepted by
        # libxml2/lxml; lenient=True wires the native set_lenient toggle to match.
        doc = P.fromstring("<u>http://example.com/a b</u>")
        strict = P.XMLSchema(P.fromstring(self.ANYURI_SCHEMA))
        lenient = P.XMLSchema(P.fromstring(self.ANYURI_SCHEMA), lenient=True)
        assert strict.validate(doc) is False
        assert lenient.validate(doc) is True

    def test_documentinvalid_error_log_is_per_instance(self):
        # A class-level list would leak appends across instances.
        a = P.DocumentInvalid("a")
        b = P.DocumentInvalid("b")
        a.error_log.append("x")
        assert a.error_log == ["x"]
        assert b.error_log == []


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

    def test_parser_rejects_unknown_kwargs(self):
        P.XMLParser(target=None, resolvers=None)
        with pytest.raises(TypeError):
            P.XMLParser(unexpected=True)

    def test_remove_comments(self):
        parser = P.XMLParser(remove_comments=True)
        root = P.fromstring("<a><!--x--><b/></a>", parser)
        assert [e.tag for e in root] == ["b"]

    def test_strip_cdata_merges_surrounding_text(self):
        parser = P.XMLParser()
        root = P.fromstring("<a>t<![CDATA[u]]>v<b/>x<![CDATA[y]]>z</a>", parser)
        assert root.text == "tuv"
        assert root[0].tail == "xyz"
        assert P.tostring(root, encoding="unicode") == "<a>tuv<b/>xyz</a>"

    def test_strip_cdata_false_exposes_contiguous_text_and_tail(self):
        # With CDATA preservation, one logical lxml .text/.tail value can be
        # split across text + CDATA + text native nodes.  The public API should
        # expose and replace the whole contiguous run as one string.
        parser = P.XMLParser(strip_cdata=False)
        root = P.fromstring(
            "<a>t<![CDATA[u]]>v<b/>x<![CDATA[y]]>z<c/></a>",
            parser,
        )

        assert root.text == "tuv"
        assert root[0].tail == "xyz"

        root.text = "lead"
        root[0].tail = "tail"
        assert root.text == "lead"
        assert root[0].tail == "tail"
        assert (
            P.tostring(root, encoding="unicode")
            == "<a>lead<b/>tail<c/></a>"
        )

        root.text = None
        root[0].tail = None
        assert root.text is None
        assert root[0].tail is None
        assert P.tostring(root, encoding="unicode") == "<a><b/><c/></a>"

    def test_huge_tree_allows_deep_nesting(self):
        depth = 500
        xml = "<r>" + "<n>" * depth + "</n>" * depth + "</r>"
        with pytest.raises(P.XMLSyntaxError):
            P.fromstring(xml)  # default depth limit blocks it
        parser = P.XMLParser(huge_tree=True)
        root = P.fromstring(xml, parser)
        assert root.tag == "r"

    def test_forbid_dtd_parser_rejects_doctype(self):
        xml = '<!DOCTYPE r SYSTEM "r.dtd"><r/>'
        # Default parser accepts (DTD ignored); forbid_dtd rejects.
        assert P.fromstring(xml).tag == "r"
        parser = P.XMLParser(forbid_dtd=True)
        with pytest.raises(P.XMLSyntaxError):
            P.fromstring(xml, parser)

    def test_forbid_entities_parser_rejects_entity_decl(self):
        xml = '<!DOCTYPE r [ <!ENTITY x "y"> ]><r>&x;</r>'
        assert P.fromstring(xml).tag == "r"
        parser = P.XMLParser(forbid_entities=True)
        with pytest.raises(P.XMLSyntaxError):
            P.fromstring(xml, parser)

    def test_forbid_entities_allows_entity_free_dtd(self):
        # Narrower than forbid_dtd: an <!ELEMENT>-only DTD is still accepted.
        xml = "<!DOCTYPE r [ <!ELEMENT r EMPTY> ]><r/>"
        parser = P.XMLParser(forbid_entities=True)
        assert P.fromstring(xml, parser).tag == "r"

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

    def test_fromstring_bom_prefixed_bytes(self):
        # In-memory parsing goes through fromstring; a UTF-8 BOM is handled.
        root = P.fromstring(b"\xef\xbb\xbf<doc>hi</doc>")
        assert root.tag == "doc"
        assert root.text == "hi"

    @pytest.mark.parametrize("encoding", ["utf-16-le", "utf-16-be"])
    def test_fromstring_utf16_without_bom(self, encoding):
        # UTF-16 (LE/BE) bytes without a BOM are decoded by the native parser.
        root = P.fromstring("<doc>hi</doc>".encode(encoding))
        assert root.tag == "doc"
        assert root.text == "hi"

    def test_parse_treats_str_and_bytes_as_path(self):
        # Like lxml, parse() interprets str/bytes as a filename, not inline XML.
        with pytest.raises(OSError):
            P.parse("<a/>")
        with pytest.raises(OSError):
            P.parse(b"<a/>")

    def test_parse_accepts_file_like(self):
        import io

        root = P.parse(io.BytesIO(b"<doc>hi</doc>")).getroot()
        assert root.tag == "doc"

    def test_empty_elementtree_convenience_methods(self):
        tree = P.ElementTree()

        for method in (
            lambda: tree.find("x"),
            lambda: tree.findall("x"),
            lambda: tree.findtext("x", default="D"),
            lambda: list(tree.iterfind("x")),
            lambda: tree.xpath("//x"),
        ):
            with pytest.raises(AssertionError, match="missing root"):
                method()

        assert list(tree.iter()) == []

    def test_empty_elementtree_write_raises_clear_error(self, tmp_path):
        import io

        tree = P.ElementTree()
        with pytest.raises(AssertionError, match="missing root"):
            P.tostring(tree, encoding="unicode")
        with pytest.raises(AssertionError, match="missing root"):
            tree.write(io.BytesIO())
        with pytest.raises(AssertionError, match="missing root"):
            tree.write(tmp_path / "empty.xml", encoding="unicode")

    def test_elementtree_schema_validation_uses_selected_root(self):
        # XMLSchema.validate() serializes the tree it receives.  A tree rooted
        # at <a/> must validate as <a/>, not as the original <r> document.
        root = P.fromstring("<r><a/><b>secret</b></r>")
        tree = P.ElementTree(root[0])
        child_schema = P.XMLSchema(
            P.fromstring(
                "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>"
                "<xs:element name='a'/>"
                "</xs:schema>"
            )
        )
        document_schema = P.XMLSchema(
            P.fromstring(
                "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>"
                "<xs:element name='r'/>"
                "</xs:schema>"
            )
        )

        assert P.tostring(tree, encoding="unicode") == "<a/>"
        assert child_schema.validate(tree) is True
        assert document_schema.validate(tree) is False

    def test_elementtree_write_unicode_path_uses_utf8(self, tmp_path):
        path = tmp_path / "unicode.xml"
        P.ElementTree(P.fromstring("<doc>caf&#233;</doc>")).write(
            path,
            encoding="unicode",
        )
        assert path.read_bytes() == "<doc>caf\u00e9</doc>".encode("utf-8")

    def test_tostring_rejects_non_xml_method(self):
        el = P.fromstring("<a>x</a>")
        assert P.tostring(el, method="xml", encoding="unicode") == "<a>x</a>"
        for method in ("html", "text", "c14n"):
            with pytest.raises(NotImplementedError):
                P.tostring(el, method=method)

    def test_tostring_rejects_unexpected_kwargs(self):
        # Unsupported lxml options (or typos) must not be silently dropped.
        el = P.fromstring("<a>x</a>")
        with pytest.raises(TypeError, match="with_tail"):
            P.tostring(el, encoding="unicode", with_tail=False)

    def test_mutation_rejects_non_element(self):
        # Passing a non-element to a mutation method raises TypeError (like
        # lxml), not a stray AttributeError, for every entry point.
        root = P.fromstring("<r><a/></r>")
        with pytest.raises(TypeError):
            root.append("x")
        with pytest.raises(TypeError):
            root.insert(0, 123)
        with pytest.raises(TypeError):
            root.replace(root[0], "x")
        with pytest.raises(TypeError):
            root[0].addnext("x")
        with pytest.raises(TypeError):
            root[0].addprevious("x")
        with pytest.raises(TypeError):
            root[0] = "x"

    def test_index_normalizes_negative_bounds(self):
        # Negative start/stop count from the end, like list.index / lxml.
        root = P.fromstring("<r><a/><b/><c/></r>")
        b = root[1]
        assert root.index(b) == 1
        assert root.index(b, -2) == 1  # search starts at index 1
        assert root.index(b, 0, -1) == 1  # b is before the last child
        with pytest.raises(ValueError):
            root.index(b, -1)  # search starts at the last child, b is earlier
        with pytest.raises(ValueError):
            root.index(b, 0, -2)  # range excludes index 1

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

    def test_constructed_xml_names_are_validated(self):
        # XML names are serialized as markup, so invalid names must be rejected
        # before they can inject tag delimiters, attributes, or bogus prefixes.
        with pytest.raises(ValueError):
            P.Element("root/><evil")
        with pytest.raises(ValueError):
            P.Element("root", {"bad name": "1"})
        with pytest.raises(ValueError):
            P.Element("root", nsmap={'p injected="1"': "urn:x"})
        with pytest.raises(ValueError):
            P.register_namespace('p injected="1"', "urn:x")
        # The xml/xmlns prefixes are reserved by the XML Namespaces spec.
        with pytest.raises(ValueError):
            P.register_namespace("xml", "urn:x")
        with pytest.raises(ValueError):
            P.register_namespace("xmlns", "urn:x")

        root = P.Element("root")
        with pytest.raises(ValueError):
            P.SubElement(root, 'x/><evil attr="1"')
        with pytest.raises(ValueError):
            root.set('a="v"/><evil foo', "z")
        with pytest.raises(ValueError):
            root.tag = 'safe injected="1"'

    def test_valid_constructed_xml_names_still_work(self):
        root = P.Element("{urn:x}root", nsmap={"x": "urn:x"})
        child = P.SubElement(root, "{urn:x}child")
        child.set("{urn:y}attr", "v")
        # The validation guard should not reject legitimate local names,
        # prefixes, default namespace handling, or namespaced attributes.
        roundtrip = P.fromstring(P.tostring(root, encoding="unicode"))
        assert roundtrip.tag == "{urn:x}root"
        assert roundtrip[0].tag == "{urn:x}child"
        assert roundtrip[0].get("{urn:y}attr") == "v"

    def test_supplementary_plane_xml_names_are_valid(self):
        local = "\U00010000name"
        prefix = "\U00010000p"
        root = P.Element("{urn:x}%s" % local, nsmap={prefix: "urn:x"})
        root.set(local, "value")

        roundtrip = P.fromstring(P.tostring(root, encoding="unicode"))
        assert roundtrip.tag == "{urn:x}%s" % local
        assert roundtrip.get(local) == "value"

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

    def test_cross_tree_move_preserves_cdata_tail(self):
        src = P.fromstring(
            "<src><item/><![CDATA[tail]]></src>",
            P.XMLParser(strip_cdata=False),
        )
        dst = P.Element("dst")
        dst.append(src[0])
        assert (
            P.tostring(dst, encoding="unicode")
            == "<dst><item/><![CDATA[tail]]></dst>"
        )

    def test_cross_tree_move_keeps_inherited_prefix(self):
        # A prefix declared on an ancestor outside the moved subtree must stay
        # declared after the move, so prefixed names serialize correctly.
        src = P.fromstring(
            '<root xmlns:a="urn:a"><keep><moveme><a:inner/></moveme></keep></root>'
        )
        dst = P.Element("dst")
        dst.append(src.find(".//moveme"))
        out = P.tostring(dst, encoding="unicode")
        # Re-parse: a:inner must still resolve to urn:a.
        moved = P.fromstring(out)[0]
        assert moved.tag == "moveme"
        assert moved[0].tag == "{urn:a}inner"

    def test_cross_tree_move_keeps_inherited_default_ns(self):
        # A default namespace inherited from an ancestor must survive the move
        # so the whole subtree stays in that namespace.
        src = P.fromstring(
            '<root xmlns="urn:d"><keep><moveme><inner/></moveme></keep></root>'
        )
        dst = P.Element("dst")
        dst.append(src.find(".//{urn:d}moveme"))
        moved = P.fromstring(P.tostring(dst, encoding="unicode"))[0]
        assert moved.tag == "{urn:d}moveme"
        assert moved[0].tag == "{urn:d}inner"

    def test_tag_setter_reuses_inherited_default_ns(self):
        # Renaming into a namespace declared as the default on an ancestor must
        # reuse that default rather than forcing a generated prefix.
        root = P.fromstring('<root xmlns="urn:d"><child/></root>')
        root[0].tag = "{urn:d}renamed"
        assert P.tostring(root, encoding="unicode") == (
            '<root xmlns="urn:d"><renamed/></root>'
        )

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

    def test_xpath_constructors_reject_unknown_kwargs(self):
        # Unsupported options (or typos) must not be silently dropped.
        root = P.fromstring("<a><b/></a>")
        with pytest.raises(TypeError):
            P.XPath("//b", smart_strings=False)
        with pytest.raises(TypeError):
            P.XPathEvaluator(root, regexp=False)

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

    def test_find_with_nsmap_default_namespace(self):
        # nsmap has a None key for the default namespace; find/findall must not
        # raise TypeError while building the selector cache key.
        root = P.fromstring(
            '<r xmlns="urn:d" xmlns:a="urn:a"><a:x/><y/></r>'
        )
        ns = root.nsmap
        assert None in ns and "a" in ns
        assert root.find(".//{urn:a}x") is not None
        assert [e.tag for e in root.findall("a:x", ns)] == ["{urn:a}x"]

    def test_generated_prefix_does_not_collide(self):
        # A parsed document already using ns0 must keep that binding; a generated
        # prefix for a new namespace must not reuse (and redeclare) ns0.
        src = P.fromstring('<r xmlns:ns0="urn:existing"><ns0:keep/></r>')
        P.SubElement(src, "{urn:new}item")
        out = P.tostring(src, encoding="unicode")
        assert 'xmlns:ns0="urn:existing"' in out  # original binding intact
        assert "urn:new" in out
        # The new namespace uses a different prefix, not ns0.
        assert 'ns0="urn:new"' not in out

    def test_xpath_text_selection_returns_strings(self):
        # A text()/CDATA node-set yields plain strings (like lxml smart-strings),
        # not _Element node proxies.
        root = P.fromstring("<r>a<b>x</b>c</r>")
        res = root.xpath("//text()")
        assert res == ["a", "x", "c"]
        assert all(isinstance(s, str) for s in res)
        # Element selections still come back as elements.
        assert [e.tag for e in root.xpath("//b")] == ["b"]

    def test_huge_tree_entity_limit_fits_usize(self):
        # huge_tree must not overflow the native usize max_entity_expansion;
        # this is a regression guard for 32-bit builds where 1<<40 > usize.
        assert P._HUGE_ENTITY <= sys.maxsize
        root = P.fromstring("<r><a/></r>", P.XMLParser(huge_tree=True))
        assert root.tag == "r"

    def test_dump_forwards_kwargs(self, capsys):
        root = P.fromstring("<a>x</a>")
        # xml_declaration reaches tostring and prepends the declaration.
        P.dump(root, xml_declaration=True)
        out = capsys.readouterr().out
        assert out.startswith('<?xml version="1.0"?>')
        assert "<a>x</a>" in out
        # Unsupported/typo kwargs are rejected by tostring, not silently dropped.
        with pytest.raises(TypeError):
            P.dump(root, bogus=True)

    def test_remove_comment_merges_surrounding_text(self):
        # Removing a comment/PI must merge the text it split, so .text/.tail
        # expose a single contiguous run as in lxml.
        root = P.fromstring("<a>t<!--c-->u<b/></a>", P.XMLParser(remove_comments=True))
        assert root.text == "tu"
        assert P.tostring(root, encoding="unicode") == "<a>tu<b/></a>"
        # Same for a removed PI splitting a tail run.
        root = P.fromstring("<a><b/>x<?pi y?>z</a>", P.XMLParser(remove_pis=True))
        assert root[0].tail == "xz"


DOCTYPE_DOC = '<!DOCTYPE root SYSTEM "r.dtd"><root><a/></root>'


@requires_lxml
class TestDoctypeDifferential:
    """DOCTYPE round-trip and docinfo vs lxml (uppsala 0.5.0)."""

    def test_docinfo_doctype_matches_lxml(self):
        pt = P.ElementTree(P.fromstring(DOCTYPE_DOC))
        lt = L.ElementTree(L.fromstring(DOCTYPE_DOC))
        assert pt.docinfo.doctype == lt.docinfo.doctype

    def test_docinfo_doctype_empty_when_absent(self):
        pt = P.ElementTree(P.fromstring("<root/>"))
        lt = L.ElementTree(L.fromstring("<root/>"))
        # lxml returns "" (not None) for a document without a DOCTYPE.
        assert pt.docinfo.doctype == lt.docinfo.doctype == ""

    def test_tostring_tree_includes_doctype(self):
        pt = P.ElementTree(P.fromstring(DOCTYPE_DOC))
        lt = L.ElementTree(L.fromstring(DOCTYPE_DOC))
        assert P.tostring(pt, encoding="unicode") == L.tostring(lt, encoding="unicode")

    def test_tostring_element_omits_doctype(self):
        pr = P.fromstring(DOCTYPE_DOC)
        lr = L.fromstring(DOCTYPE_DOC)
        # Serializing a bare element never emits the DOCTYPE in lxml.
        assert P.tostring(pr, encoding="unicode") == L.tostring(lr, encoding="unicode")

    def test_tostring_explicit_doctype_kwarg(self):
        pr = P.fromstring("<root><a/></root>")
        lr = L.fromstring("<root><a/></root>")
        assert P.tostring(pr, encoding="unicode", doctype="<!DOCTYPE x>") == L.tostring(
            lr, encoding="unicode", doctype="<!DOCTYPE x>"
        )


class TestDoctypeStandalone:
    """DOCTYPE behavior that does not depend on lxml."""

    def test_docinfo_doctype_value(self):
        tree = P.ElementTree(P.fromstring(DOCTYPE_DOC))
        assert tree.docinfo.doctype == '<!DOCTYPE root SYSTEM "r.dtd">'

    def test_tree_round_trips_doctype(self):
        tree = P.ElementTree(P.fromstring(DOCTYPE_DOC))
        assert (
            P.tostring(tree, encoding="unicode")
            == '<!DOCTYPE root SYSTEM "r.dtd">\n<root><a/></root>'
        )

    def test_explicit_doctype_overrides_preserved(self):
        tree = P.ElementTree(P.fromstring(DOCTYPE_DOC))
        out = P.tostring(tree, encoding="unicode", doctype="<!DOCTYPE other>")
        assert out == "<!DOCTYPE other>\n<root><a/></root>"


# ---------------------------------------------------------------------------
# XSLT 1.0 transformation
# ---------------------------------------------------------------------------

# An identity-ish stylesheet that strips ds:Signature and drops empty-text
# whitespace nodes -- the shape pyFF's tidy.xsl relies on.
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

XSLT_DOC = (
    '<root xmlns:ds="http://www.w3.org/2000/09/xmldsig#">'
    "<a>keep</a><ds:Signature>drop</ds:Signature><b>also</b>"
    "</root>"
)


class TestXSLT:
    @requires_lxml
    def test_tidy_matches_lxml(self):
        pres = P.XSLT(P.fromstring(TIDY_XSLT))(P.fromstring(XSLT_DOC)).getroot()
        lres = L.XSLT(L.fromstring(TIDY_XSLT.encode()))(L.fromstring(XSLT_DOC.encode())).getroot()
        assert L.canonicalize(P.tostring(pres, encoding="unicode")) == L.canonicalize(
            L.tostring(lres).decode()
        )

    @requires_lxml
    def test_value_of_matches_lxml(self):
        ss = (
            '<xsl:stylesheet version="1.0"'
            ' xmlns:xsl="http://www.w3.org/1999/XSL/Transform">'
            '<xsl:output method="xml" omit-xml-declaration="yes"/>'
            '<xsl:template match="/"><out><xsl:value-of select="/a"/></out></xsl:template>'
            "</xsl:stylesheet>"
        )
        p = str(P.XSLT(P.fromstring(ss))(P.fromstring("<a>hi</a>")))
        assert "<out>hi</out>" in p

    def test_result_getroot_and_str(self):
        result = P.XSLT(P.fromstring(TIDY_XSLT))(P.fromstring(XSLT_DOC))
        # Signature stripped, two element children remain.
        root = result.getroot()
        kids = [c.tag for c in root]
        assert kids == ["a", "b"]
        assert isinstance(str(result), str)
        assert isinstance(bytes(result), bytes)

    def test_error_log_empty_on_success(self):
        t = P.XSLT(P.fromstring(TIDY_XSLT))
        t(P.fromstring(XSLT_DOC))
        assert t.error_log == []

    def test_bad_stylesheet_raises_parse_error(self):
        with pytest.raises(P.XSLTParseError):
            P.XSLT(P.fromstring("<notxsl/>"))

    def test_params_not_supported(self):
        t = P.XSLT(P.fromstring(TIDY_XSLT))
        with pytest.raises(NotImplementedError):
            t(P.fromstring(XSLT_DOC), some_param="x")

    def test_regexp_false_rejected(self):
        # EXSLT regexp is always on; an explicit request to disable it must not
        # be silently ignored.
        with pytest.raises(NotImplementedError):
            P.XSLT(P.fromstring(TIDY_XSLT), regexp=False)

    def test_profile_run_rejected(self):
        # There is no profiler; profile_run=True must not silently no-op.
        t = P.XSLT(P.fromstring(TIDY_XSLT))
        with pytest.raises(NotImplementedError):
            t(P.fromstring(XSLT_DOC), profile_run=True)

    def test_xslt_exception_hierarchy(self):
        assert issubclass(P.XSLTParseError, P.XSLTError)
        assert issubclass(P.XSLTApplyError, P.XSLTError)
        assert issubclass(P.XSLTError, P.LxmlError)


class TestSmartStringsCompat:
    """lxml's smart_strings kwarg is accepted and ignored (pyFF passes it)."""

    NS = {"md": "urn:oasis:names:tc:SAML:2.0:metadata"}
    DOC = (
        '<md:EntitiesDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata">'
        '<md:EntityDescriptor entityID="https://a"/>'
        '<md:EntityDescriptor entityID="https://b"/>'
        "</md:EntitiesDescriptor>"
    )

    def test_element_xpath_accepts_smart_strings(self):
        root = P.fromstring(self.DOC)
        res = root.xpath("//md:EntityDescriptor", namespaces=self.NS, smart_strings=False)
        assert len(res) == 2

    def test_attribute_xpath_returns_plain_str(self):
        root = P.fromstring(self.DOC)
        ids = root.xpath(
            "//md:EntityDescriptor/@entityID", namespaces=self.NS, smart_strings=False
        )
        assert ids == ["https://a", "https://b"]
        assert all(type(x) is str for x in ids)

    def test_tree_xpath_accepts_smart_strings(self):
        tree = P.ElementTree(P.fromstring(self.DOC))
        res = tree.xpath("//md:EntityDescriptor", namespaces=self.NS, smart_strings=False)
        assert len(res) == 2

    def test_variables_still_rejected(self):
        root = P.fromstring(self.DOC)
        with pytest.raises(NotImplementedError):
            root.xpath("//md:EntityDescriptor[@entityID=$e]", namespaces=self.NS, e="https://a")


# ---------------------------------------------------------------------------
# XInclude 1.0
# ---------------------------------------------------------------------------


def _write(p, name, content):
    f = p / name
    f.write_text(content, encoding="utf-8")
    return f


class TestXInclude:
    def _setup(self, tmp_path):
        _write(tmp_path, "inc1.xml", '<item id="1"><name>FR</name></item>')
        _write(tmp_path, "inc2.xml", '<item id="2"><name>GR</name></item>')
        _write(tmp_path, "note.txt", "plain text")
        main = (
            '<list xmlns:xi="http://www.w3.org/2001/XInclude">'
            '<xi:include href="inc1.xml"/>tail1'
            '<xi:include href="inc2.xml"/>'
            '<xi:include href="note.txt" parse="text"/>'
            '<xi:include href="missing.xml"><xi:fallback><item id="fb"/></xi:fallback></xi:include>'
            "</list>"
        )
        return _write(tmp_path, "main.xml", main)

    @requires_lxml
    def test_xinclude_matches_lxml(self, tmp_path):
        main = str(self._setup(tmp_path))
        pt = P.parse(main)
        pt.xinclude()
        lt = L.parse(main)
        lt.xinclude()
        assert L.canonicalize(P.tostring(pt.getroot(), encoding="unicode")) == L.canonicalize(
            L.tostring(lt.getroot()).decode()
        )

    def test_xinclude_xml_and_text_and_fallback(self, tmp_path):
        main = str(self._setup(tmp_path))
        tree = P.parse(main)
        tree.xinclude()
        root = tree.getroot()
        ids = [c.get("id") for c in root]
        assert ids == ["1", "2", "fb"]  # two xml includes + fallback element
        assert "plain text" in P.tostring(root, encoding="unicode")
        # no xi:include elements remain
        assert root.find("{http://www.w3.org/2001/XInclude}include") is None

    def test_xinclude_missing_without_fallback_raises(self, tmp_path):
        main = _write(
            tmp_path,
            "bad.xml",
            '<list xmlns:xi="http://www.w3.org/2001/XInclude">'
            '<xi:include href="nope.xml"/></list>',
        )
        tree = P.parse(str(main))
        with pytest.raises(P.XIncludeError):
            tree.xinclude()

    def test_xinclude_nested(self, tmp_path):
        _write(tmp_path, "leaf.xml", "<leaf/>")
        _write(
            tmp_path,
            "mid.xml",
            '<mid xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="leaf.xml"/></mid>',
        )
        main = _write(
            tmp_path,
            "top.xml",
            '<top xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="mid.xml"/></top>',
        )
        tree = P.parse(str(main))
        tree.xinclude()
        assert tree.getroot().find(".//leaf") is not None

    def test_xinclude_network_blocked_by_default(self):
        # Remote fetches are opt-in (anti-SSRF): a bare remote include with no
        # fallback raises rather than performing the network request.
        root = P.fromstring(
            '<r xmlns:xi="http://www.w3.org/2001/XInclude">'
            '<xi:include href="http://169.254.169.254/x"/></r>'
        )
        with pytest.raises(P.XIncludeError):
            root.xinclude()

    def test_xinclude_network_blocked_uses_fallback(self):
        # A blocked remote target behaves like any other load failure, so its
        # xi:fallback content is spliced in instead.
        root = P.fromstring(
            '<r xmlns:xi="http://www.w3.org/2001/XInclude">'
            '<xi:include href="http://169.254.169.254/x">'
            "<xi:fallback>SAFE</xi:fallback></xi:include></r>"
        )
        root.xinclude()
        assert root.text == "SAFE"


class TestNamespaceSerializationAndCopy:
    """Sub-element serialization keeps inherited namespaces; deepcopy works."""

    DOC = (
        '<md:R xmlns:md="urn:md" xmlns:shibmd="urn:shib">'
        '<md:E entityID="x"><md:X><shibmd:Scope>a</shibmd:Scope></md:X></md:E>'
        "</md:R>"
    )

    def test_subelement_serialization_round_trips(self):
        sub = P.fromstring(self.DOC)[0]  # md:E using inherited shibmd prefix
        s = P.tostring(sub, encoding="unicode")
        assert "xmlns:shibmd" in s
        # must reparse standalone (the bug that broke pyFF)
        P.fromstring(s)

    @requires_lxml
    def test_subelement_serialization_matches_lxml(self):
        ps = P.tostring(P.fromstring(self.DOC)[0], encoding="unicode")
        ls = L.tostring(L.fromstring(self.DOC.encode())[0]).decode()
        assert L.canonicalize(ps) == L.canonicalize(ls)

    def test_root_serialization_no_duplicate_ns(self):
        root = P.fromstring('<r xmlns:p="urn:p"><p:c/></r>')
        assert P.tostring(root, encoding="unicode") == '<r xmlns:p="urn:p"><p:c/></r>'

    def test_deepcopy_is_detached_clone(self):
        import copy

        root = P.fromstring(self.DOC)
        ent = root[0]
        clone = copy.deepcopy(ent)
        assert clone.get("entityID") == "x"
        assert clone.getparent() is None  # detached
        # mutating the clone does not touch the original
        clone.set("entityID", "y")
        assert ent.get("entityID") == "x"
        # clone serializes standalone (inherited ns preserved)
        P.fromstring(P.tostring(clone, encoding="unicode"))

    def test_document_invalid_has_error_log(self):
        assert P.DocumentInvalid("x").error_log == []
