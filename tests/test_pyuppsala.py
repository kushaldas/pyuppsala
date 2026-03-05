"""Comprehensive tests for pyuppsala — Python bindings for the Uppsala XML library."""

import pytest
import pyuppsala
from pyuppsala import (
    Document,
    Node,
    QName,
    Attribute,
    XPathEvaluator,
    XsdValidator,
    ValidationError,
    XmlWriter,
    XsdRegex,
    parse,
    parse_bytes,
    XmlParseError,
    XmlWellFormednessError,
    XmlNamespaceError,
    XPathError,
    XsdValidationError,
)


# ============================================================================
# Module-level functions
# ============================================================================


class TestParse:
    def test_parse_simple(self):
        doc = parse("<root/>")
        assert doc is not None
        assert doc.document_element is not None

    def test_parse_with_content(self):
        doc = parse("<root><child>text</child></root>")
        el = doc.document_element
        assert el.tag.local_name == "root"

    def test_parse_invalid_xml(self):
        with pytest.raises(XmlParseError):
            parse("<root>")

    def test_parse_empty_string(self):
        with pytest.raises((XmlParseError, XmlWellFormednessError)):
            parse("")

    def test_parse_bytes_utf8(self):
        data = b"<root>hello</root>"
        doc = parse_bytes(data)
        assert doc.document_element.text_content == "hello"

    def test_parse_bytes_utf16_le(self):
        xml = '<?xml version="1.0" encoding="UTF-16"?><root>ok</root>'
        data = b"\xff\xfe" + xml.encode("utf-16-le")
        doc = parse_bytes(data)
        assert doc.document_element.text_content == "ok"

    def test_parse_bytes_utf16_be(self):
        xml = '<?xml version="1.0" encoding="UTF-16"?><root>ok</root>'
        data = b"\xfe\xff" + xml.encode("utf-16-be")
        doc = parse_bytes(data)
        assert doc.document_element.text_content == "ok"


# ============================================================================
# Document
# ============================================================================


class TestDocument:
    def test_empty_document(self):
        doc = Document.empty()
        assert doc.document_element is None
        assert doc.root is not None

    def test_root_is_document_node(self):
        doc = parse("<root/>")
        assert doc.root.kind == "document"

    def test_document_element(self):
        doc = parse("<root/>")
        el = doc.document_element
        assert el is not None
        assert el.kind == "element"
        assert el.tag.local_name == "root"

    def test_repr(self):
        doc = parse("<root/>")
        assert "root" in repr(doc)

    def test_str_serialization(self):
        doc = parse("<root/>")
        assert str(doc) == "<root/>"

    def test_bool(self):
        doc = parse("<root/>")
        assert bool(doc) is True

    def test_get_elements_by_tag_name(self):
        doc = parse("<root><a/><b><a/></b></root>")
        nodes = doc.get_elements_by_tag_name("a")
        assert len(nodes) == 2

    def test_get_elements_by_tag_name_ns(self):
        doc = parse('<root xmlns:ns="urn:test"><ns:item/><item/></root>')
        nodes = doc.get_elements_by_tag_name_ns("urn:test", "item")
        assert len(nodes) == 1

    # -- Tree mutation --------------------------------------------------------

    def test_create_element(self):
        doc = Document.empty()
        root = doc.create_element("root")
        doc.append_child(doc.root, root)
        assert doc.document_element.tag.local_name == "root"

    def test_create_element_with_namespace(self):
        doc = Document.empty()
        el = doc.create_element("item", namespace_uri="urn:test", prefix="ns")
        assert el.tag.namespace_uri == "urn:test"
        assert el.tag.prefix == "ns"

    def test_create_text(self):
        doc = parse("<root/>")
        text = doc.create_text("hello")
        doc.append_child(doc.document_element, text)
        assert doc.document_element.text_content == "hello"

    def test_create_comment(self):
        doc = parse("<root/>")
        comment = doc.create_comment("a comment")
        doc.append_child(doc.document_element, comment)
        assert "<!--a comment-->" in doc.to_xml()

    def test_create_cdata(self):
        doc = parse("<root/>")
        cdata = doc.create_cdata("some <data>")
        doc.append_child(doc.document_element, cdata)
        assert "<![CDATA[some <data>]]>" in doc.to_xml()

    def test_create_processing_instruction(self):
        doc = parse("<root/>")
        pi = doc.create_processing_instruction("target", "data")
        doc.append_child(doc.document_element, pi)
        assert "<?target data?>" in doc.to_xml()

    def test_create_processing_instruction_no_data(self):
        doc = parse("<root/>")
        pi = doc.create_processing_instruction("target", None)
        doc.append_child(doc.document_element, pi)
        assert "<?target?>" in doc.to_xml()

    def test_append_child(self):
        doc = parse("<root/>")
        child = doc.create_element("child")
        doc.append_child(doc.document_element, child)
        assert len(doc.document_element) == 1
        assert doc.document_element[0].tag.local_name == "child"

    def test_insert_before(self):
        doc = parse("<root><b/></root>")
        a = doc.create_element("a")
        root = doc.document_element
        b = root.children[0]
        doc.insert_before(root, a, b)
        children = root.children
        assert children[0].tag.local_name == "a"
        assert children[1].tag.local_name == "b"

    def test_insert_after(self):
        doc = parse("<root><a/></root>")
        b = doc.create_element("b")
        root = doc.document_element
        a = root.children[0]
        doc.insert_after(root, b, a)
        children = root.children
        assert children[0].tag.local_name == "a"
        assert children[1].tag.local_name == "b"

    def test_remove_child(self):
        doc = parse("<root><child/></root>")
        root = doc.document_element
        child = root.children[0]
        doc.remove_child(root, child)
        assert len(root.children) == 0

    def test_replace_child(self):
        doc = parse("<root><old/></root>")
        root = doc.document_element
        old = root.children[0]
        new = doc.create_element("new")
        doc.replace_child(root, new, old)
        assert root.children[0].tag.local_name == "new"

    def test_detach(self):
        doc = parse("<root><child>text</child></root>")
        root = doc.document_element
        child = root.children[0]
        doc.detach(child)
        assert len(root.children) == 0
        # Re-attach
        doc.append_child(root, child)
        assert len(root.children) == 1
        assert root.children[0].tag.local_name == "child"

    # -- Serialization --------------------------------------------------------

    def test_to_xml(self):
        doc = parse("<root><child/></root>")
        assert doc.to_xml() == "<root><child/></root>"

    def test_to_xml_with_options_pretty(self):
        doc = parse("<root><child/></root>")
        pretty = doc.to_xml_with_options(indent="  ")
        assert "\n" in pretty
        assert "  <child/>" in pretty

    def test_to_xml_with_options_expand_empty(self):
        doc = parse("<root/>")
        result = doc.to_xml_with_options(expand_empty_elements=True)
        assert result == "<root></root>"

    def test_write_to_file(self, tmp_path):
        doc = parse("<root><child/></root>")
        path = str(tmp_path / "output.xml")
        doc.write_to_file(path)
        with open(path) as f:
            assert f.read() == "<root><child/></root>"

    # -- XPath preparation ----------------------------------------------------

    def test_prepare_xpath(self):
        doc = parse("<root/>")
        doc.prepare_xpath()  # Should not raise


# ============================================================================
# Node
# ============================================================================


class TestNode:
    def test_kind_element(self):
        doc = parse("<root/>")
        assert doc.document_element.kind == "element"

    def test_kind_text(self):
        doc = parse("<root>hello</root>")
        child = doc.document_element.children[0]
        assert child.kind == "text"

    def test_kind_comment(self):
        doc = parse("<root><!--c--></root>")
        child = doc.document_element.children[0]
        assert child.kind == "comment"

    def test_kind_cdata(self):
        doc = parse("<root><![CDATA[data]]></root>")
        child = doc.document_element.children[0]
        assert child.kind == "cdata"

    def test_kind_pi(self):
        doc = parse("<root><?pi data?></root>")
        child = doc.document_element.children[0]
        assert child.kind == "processing_instruction"

    def test_tag_for_element(self):
        doc = parse("<root/>")
        assert doc.document_element.tag.local_name == "root"

    def test_tag_for_non_element(self):
        doc = parse("<root>text</root>")
        text_node = doc.document_element.children[0]
        assert text_node.tag is None

    def test_text_for_text_node(self):
        doc = parse("<root>hello</root>")
        text_node = doc.document_element.children[0]
        assert text_node.text == "hello"

    def test_text_content_deep(self):
        doc = parse("<root>a<child>b</child>c</root>")
        assert doc.document_element.text_content == "abc"

    def test_attributes(self):
        doc = parse('<root a="1" b="2"/>')
        attrs = doc.document_element.attributes
        assert len(attrs) == 2
        names = {a.name.local_name for a in attrs}
        assert names == {"a", "b"}

    def test_get_attribute(self):
        doc = parse('<root key="val"/>')
        assert doc.document_element.get_attribute("key") == "val"
        assert doc.document_element.get_attribute("missing") is None

    def test_get_attribute_ns(self):
        doc = parse('<root xmlns:ns="urn:test" ns:key="val"/>')
        assert (
            doc.document_element.get_attribute("key", namespace_uri="urn:test") == "val"
        )

    def test_set_attribute(self):
        doc = parse("<root/>")
        old = doc.document_element.set_attribute("key", "val")
        assert old is None
        assert doc.document_element.get_attribute("key") == "val"

    def test_set_attribute_replace(self):
        doc = parse('<root key="old"/>')
        old = doc.document_element.set_attribute("key", "new")
        assert old == "old"
        assert doc.document_element.get_attribute("key") == "new"

    def test_set_attribute_with_namespace(self):
        doc = parse("<root/>")
        doc.document_element.set_attribute(
            "key", "val", namespace_uri="urn:test", prefix="ns"
        )
        assert (
            doc.document_element.get_attribute("key", namespace_uri="urn:test") == "val"
        )

    def test_remove_attribute(self):
        doc = parse('<root key="val"/>')
        old = doc.document_element.remove_attribute("key")
        assert old == "val"
        assert doc.document_element.get_attribute("key") is None

    def test_remove_attribute_missing(self):
        doc = parse("<root/>")
        assert doc.document_element.remove_attribute("missing") is None

    def test_parent(self):
        doc = parse("<root><child/></root>")
        child = doc.document_element.children[0]
        parent = child.parent
        assert parent.tag.local_name == "root"

    def test_parent_of_root_element(self):
        doc = parse("<root/>")
        parent = doc.document_element.parent
        assert parent.kind == "document"

    def test_children(self):
        doc = parse("<root><a/><b/><c/></root>")
        children = doc.document_element.children
        assert len(children) == 3
        assert children[0].tag.local_name == "a"
        assert children[2].tag.local_name == "c"

    def test_line_and_column(self):
        doc = parse("<root>\n  <child/>\n</root>")
        # First child is a text node ("\n  "), second is <child/>
        children = doc.document_element.children
        child_el = [c for c in children if c.kind == "element"][0]
        # line/column are available and return non-negative integers
        assert isinstance(child_el.line, int)
        assert isinstance(child_el.column, int)
        assert child_el.line >= 1

    def test_to_xml(self):
        doc = parse("<root><child>text</child></root>")
        child = doc.document_element.children[0]
        assert child.to_xml() == "<child>text</child>"

    def test_to_xml_with_options(self):
        doc = parse("<root><child><sub/></child></root>")
        child = doc.document_element.children[0]
        pretty = child.to_xml_with_options(indent="  ")
        assert "\n" in pretty

    def test_get_elements_by_tag_name(self):
        doc = parse("<root><a><b/></a><b/></root>")
        nodes = doc.document_element.get_elements_by_tag_name("b")
        assert len(nodes) == 2

    def test_get_elements_by_tag_name_ns(self):
        doc = parse('<root xmlns:ns="urn:test"><ns:x/><x/></root>')
        nodes = doc.document_element.get_elements_by_tag_name_ns("urn:test", "x")
        assert len(nodes) == 1

    # -- Dunder methods -------------------------------------------------------

    def test_repr_element(self):
        doc = parse("<root/>")
        assert "root" in repr(doc.document_element)

    def test_repr_text(self):
        doc = parse("<root>hello</root>")
        text = doc.document_element.children[0]
        assert "text=" in repr(text)

    def test_repr_comment(self):
        doc = parse("<root><!--comm--></root>")
        c = doc.document_element.children[0]
        assert "comment=" in repr(c)

    def test_repr_document(self):
        doc = parse("<root/>")
        assert "document" in repr(doc.root)

    def test_str(self):
        doc = parse("<root/>")
        assert str(doc.document_element) == "<root/>"

    def test_len(self):
        doc = parse("<root><a/><b/></root>")
        assert len(doc.document_element) == 2

    def test_iter(self):
        doc = parse("<root><a/><b/><c/></root>")
        names = [n.tag.local_name for n in doc.document_element]
        assert names == ["a", "b", "c"]

    def test_getitem(self):
        doc = parse("<root><a/><b/><c/></root>")
        root = doc.document_element
        assert root[0].tag.local_name == "a"
        assert root[1].tag.local_name == "b"
        assert root[-1].tag.local_name == "c"

    def test_getitem_out_of_range(self):
        doc = parse("<root><a/></root>")
        with pytest.raises(IndexError):
            doc.document_element[5]

    def test_bool(self):
        doc = parse("<root/>")
        assert bool(doc.document_element) is True


# ============================================================================
# QName
# ============================================================================


class TestQName:
    def test_local_only(self):
        q = QName("root")
        assert q.local_name == "root"
        assert q.namespace_uri is None
        assert q.prefix is None

    def test_with_namespace(self):
        q = QName("item", namespace_uri="urn:test")
        assert q.namespace_uri == "urn:test"

    def test_with_prefix(self):
        q = QName("item", namespace_uri="urn:test", prefix="ns")
        assert q.prefix == "ns"
        assert q.prefixed_name == "ns:item"

    def test_prefixed_name_no_prefix(self):
        q = QName("root")
        assert q.prefixed_name == "root"

    def test_eq(self):
        a = QName("item", namespace_uri="urn:test")
        b = QName("item", namespace_uri="urn:test")
        assert a == b

    def test_eq_different_prefix_same_ns(self):
        a = QName("item", namespace_uri="urn:test", prefix="a")
        b = QName("item", namespace_uri="urn:test", prefix="b")
        assert a == b  # equality is by local_name + namespace_uri

    def test_neq(self):
        a = QName("item", namespace_uri="urn:a")
        b = QName("item", namespace_uri="urn:b")
        assert a != b

    def test_hash(self):
        a = QName("item", namespace_uri="urn:test")
        b = QName("item", namespace_uri="urn:test")
        assert hash(a) == hash(b)
        s = {a, b}
        assert len(s) == 1

    def test_repr_simple(self):
        q = QName("root")
        assert repr(q) == "QName('root')"

    def test_repr_with_ns(self):
        q = QName("item", namespace_uri="urn:test")
        assert "urn:test" in repr(q)

    def test_str(self):
        q = QName("item", prefix="ns")
        assert str(q) == "ns:item"


# ============================================================================
# Attribute
# ============================================================================


class TestAttribute:
    def test_attribute_from_parsed(self):
        doc = parse('<root key="val"/>')
        attr = doc.document_element.attributes[0]
        assert attr.name.local_name == "key"
        assert attr.value == "val"

    def test_repr(self):
        doc = parse('<root key="val"/>')
        attr = doc.document_element.attributes[0]
        assert "key" in repr(attr)
        assert "val" in repr(attr)

    def test_str(self):
        doc = parse('<root key="val"/>')
        attr = doc.document_element.attributes[0]
        assert str(attr) == 'key="val"'


# ============================================================================
# XPathEvaluator
# ============================================================================


class TestXPathEvaluator:
    def test_basic_select(self):
        doc = parse("<root><a/><b/></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        nodes = xpath.select(doc, "/root/a")
        assert len(nodes) == 1
        assert nodes[0].tag.local_name == "a"

    def test_evaluate_nodeset(self):
        doc = parse("<root><item/><item/></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        result = xpath.evaluate(doc, "/root/item")
        assert isinstance(result, list)
        assert len(result) == 2

    def test_evaluate_string(self):
        doc = parse("<root>hello</root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        result = xpath.evaluate(doc, "string(/root)")
        assert result == "hello"

    def test_evaluate_number(self):
        doc = parse("<root><a/><a/><a/></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        result = xpath.evaluate(doc, "count(/root/a)")
        assert result == 3.0

    def test_evaluate_boolean(self):
        doc = parse("<root><a/></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        result = xpath.evaluate(doc, "boolean(/root/a)")
        assert result is True

    def test_namespace_aware(self):
        doc = parse('<root xmlns:ns="urn:test"><ns:item/></root>')
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        xpath.add_namespace("ns", "urn:test")
        nodes = xpath.select(doc, "/root/ns:item")
        assert len(nodes) == 1

    def test_context_node(self):
        doc = parse("<root><a><b/></a></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        a = doc.get_elements_by_tag_name("a")[0]
        nodes = xpath.select(doc, "b", context=a)
        assert len(nodes) == 1
        assert nodes[0].tag.local_name == "b"

    def test_invalid_xpath(self):
        doc = parse("<root/>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        with pytest.raises(XPathError):
            xpath.evaluate(doc, "///invalid[")

    def test_predicate(self):
        doc = parse("<root><a id='1'/><a id='2'/></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        nodes = xpath.select(doc, "/root/a[@id='2']")
        assert len(nodes) == 1

    def test_descendant(self):
        doc = parse("<root><a><b><c/></b></a></root>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        nodes = xpath.select(doc, "//c")
        assert len(nodes) == 1

    def test_repr(self):
        xpath = XPathEvaluator()
        assert repr(xpath) == "XPathEvaluator()"


# ============================================================================
# XsdValidator
# ============================================================================


SIMPLE_SCHEMA = """\
<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="root">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="name" type="xs:string"/>
        <xs:element name="age" type="xs:integer"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>
"""


class TestXsdValidator:
    def test_valid_document(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        errors = validator.validate_str("<root><name>Alice</name><age>30</age></root>")
        assert len(errors) == 0

    def test_invalid_document(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        errors = validator.validate_str("<root><name>Alice</name></root>")
        assert len(errors) > 0

    def test_is_valid(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        doc = parse("<root><name>Alice</name><age>30</age></root>")
        assert validator.is_valid(doc) is True

    def test_is_valid_false(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        doc = parse("<root><name>Alice</name></root>")
        assert validator.is_valid(doc) is False

    def test_is_valid_str(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        assert (
            validator.is_valid_str("<root><name>Alice</name><age>30</age></root>")
            is True
        )

    def test_is_valid_str_malformed(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        # Malformed XML should return False, not raise
        assert validator.is_valid_str("<root>") is False

    def test_validate_with_document(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        doc = parse("<root><name>Alice</name><age>30</age></root>")
        errors = validator.validate(doc)
        assert len(errors) == 0

    def test_validation_error_details(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        errors = validator.validate_str("<root><wrong/></root>")
        assert len(errors) > 0
        err = errors[0]
        assert isinstance(err.message, str)
        assert len(err.message) > 0

    def test_validation_error_repr(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        errors = validator.validate_str("<root><wrong/></root>")
        r = repr(errors[0])
        assert "ValidationError" in r

    def test_validation_error_str(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        errors = validator.validate_str("<root><wrong/></root>")
        s = str(errors[0])
        assert len(s) > 0

    def test_invalid_schema(self):
        with pytest.raises(XmlParseError):
            XsdValidator("<not valid xml")

    def test_set_enforce_qname_length_facets(self):
        schema = """\
<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="root" type="xs:string"/>
</xs:schema>
"""
        validator = XsdValidator(schema)
        validator.set_enforce_qname_length_facets(False)
        # Should still work for basic types
        assert validator.is_valid_str("<root>hello</root>") is True

    def test_facet_validation(self):
        schema = """\
<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="code">
    <xs:simpleType>
      <xs:restriction base="xs:string">
        <xs:minLength value="3"/>
        <xs:maxLength value="5"/>
      </xs:restriction>
    </xs:simpleType>
  </xs:element>
</xs:schema>
"""
        validator = XsdValidator(schema)
        assert validator.is_valid_str("<code>abc</code>") is True
        assert validator.is_valid_str("<code>ab</code>") is False
        assert validator.is_valid_str("<code>abcdef</code>") is False

    def test_pattern_validation(self):
        schema = """\
<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="zip">
    <xs:simpleType>
      <xs:restriction base="xs:string">
        <xs:pattern value="[0-9]{5}"/>
      </xs:restriction>
    </xs:simpleType>
  </xs:element>
</xs:schema>
"""
        validator = XsdValidator(schema)
        assert validator.is_valid_str("<zip>12345</zip>") is True
        assert validator.is_valid_str("<zip>abcde</zip>") is False
        assert validator.is_valid_str("<zip>1234</zip>") is False

    def test_repr(self):
        validator = XsdValidator(SIMPLE_SCHEMA)
        assert repr(validator) == "XsdValidator(...)"


# ============================================================================
# XmlWriter
# ============================================================================


class TestXmlWriter:
    def test_simple_element(self):
        w = XmlWriter()
        w.start_element("root")
        w.end_element("root")
        assert w.to_string() == "<root></root>"

    def test_with_declaration(self):
        w = XmlWriter()
        w.write_declaration()
        w.start_element("root")
        w.end_element("root")
        result = w.to_string()
        assert result.startswith('<?xml version="1.0" encoding="UTF-8"?>')

    def test_declaration_full(self):
        w = XmlWriter()
        w.write_declaration_full("1.0", "UTF-8", True)
        result = w.to_string()
        assert 'standalone="yes"' in result

    def test_attributes(self):
        w = XmlWriter()
        w.start_element("root", [("key", "val")])
        w.end_element("root")
        assert 'key="val"' in w.to_string()

    def test_text(self):
        w = XmlWriter()
        w.start_element("root")
        w.text("hello & world")
        w.end_element("root")
        assert "<root>hello &amp; world</root>" == w.to_string()

    def test_cdata(self):
        w = XmlWriter()
        w.start_element("root")
        w.cdata("data")
        w.end_element("root")
        assert "<![CDATA[data]]>" in w.to_string()

    def test_comment(self):
        w = XmlWriter()
        w.start_element("root")
        w.comment("a comment")
        w.end_element("root")
        assert "<!--a comment-->" in w.to_string()

    def test_processing_instruction(self):
        w = XmlWriter()
        w.processing_instruction("target", "data")
        assert "<?target data?>" in w.to_string()

    def test_processing_instruction_no_data(self):
        w = XmlWriter()
        w.processing_instruction("target", None)
        assert "<?target?>" in w.to_string()

    def test_empty_element(self):
        w = XmlWriter()
        w.empty_element("br")
        assert w.to_string() == "<br/>"

    def test_empty_element_expanded(self):
        w = XmlWriter()
        w.empty_element_expanded("br")
        assert w.to_string() == "<br></br>"

    def test_raw(self):
        w = XmlWriter()
        w.raw("<raw>content</raw>")
        assert w.to_string() == "<raw>content</raw>"

    def test_to_bytes(self):
        w = XmlWriter()
        w.start_element("root")
        w.end_element("root")
        data = w.to_bytes()
        assert isinstance(data, bytes)
        assert data == b"<root></root>"

    def test_str(self):
        w = XmlWriter()
        w.start_element("root")
        w.end_element("root")
        assert str(w) == "<root></root>"

    def test_repr(self):
        w = XmlWriter()
        w.start_element("root")
        w.end_element("root")
        assert "XmlWriter" in repr(w)
        assert "len=" in repr(w)

    def test_len(self):
        w = XmlWriter()
        assert len(w) == 0
        w.start_element("a")
        w.end_element("a")
        assert len(w) > 0

    def test_bool_empty(self):
        w = XmlWriter()
        assert bool(w) is False

    def test_bool_nonempty(self):
        w = XmlWriter()
        w.start_element("a")
        w.end_element("a")
        assert bool(w) is True

    def test_complex_document(self):
        w = XmlWriter()
        w.write_declaration()
        w.start_element("root", [("xmlns", "urn:test")])
        w.start_element("item", [("id", "1")])
        w.text("First")
        w.end_element("item")
        w.start_element("item", [("id", "2")])
        w.text("Second")
        w.end_element("item")
        w.end_element("root")
        result = w.to_string()
        # Parse it back to verify it's valid XML
        doc = parse(result)
        items = doc.get_elements_by_tag_name("item")
        assert len(items) == 2


# ============================================================================
# XsdRegex
# ============================================================================


class TestXsdRegex:
    def test_simple_match(self):
        r = XsdRegex("[0-9]+")
        assert r.is_match("12345") is True
        assert r.is_match("abc") is False

    def test_full_match(self):
        """XSD regex is implicitly anchored — must match the entire string."""
        r = XsdRegex("[0-9]+")
        assert r.is_match("123abc") is False

    def test_pattern_property(self):
        r = XsdRegex("[a-z]+")
        assert r.pattern == "[a-z]+"

    def test_alternation(self):
        r = XsdRegex("cat|dog")
        assert r.is_match("cat") is True
        assert r.is_match("dog") is True
        assert r.is_match("bird") is False

    def test_quantifiers(self):
        r = XsdRegex("a{3}")
        assert r.is_match("aaa") is True
        assert r.is_match("aa") is False
        assert r.is_match("aaaa") is False

    def test_range_quantifier(self):
        r = XsdRegex("a{2,4}")
        assert r.is_match("a") is False
        assert r.is_match("aa") is True
        assert r.is_match("aaa") is True
        assert r.is_match("aaaa") is True
        assert r.is_match("aaaaa") is False

    def test_character_class(self):
        r = XsdRegex("[A-Za-z_][A-Za-z0-9_]*")
        assert r.is_match("hello") is True
        assert r.is_match("_var") is True
        assert r.is_match("123") is False

    def test_negated_class(self):
        r = XsdRegex("[^0-9]+")
        assert r.is_match("abc") is True
        assert r.is_match("123") is False

    def test_dot(self):
        r = XsdRegex("a.c")
        assert r.is_match("abc") is True
        assert r.is_match("a1c") is True
        assert r.is_match("ac") is False

    def test_unicode_category(self):
        r = XsdRegex("\\p{Lu}+")
        assert r.is_match("ABC") is True
        assert r.is_match("abc") is False

    def test_multi_char_escape(self):
        r = XsdRegex("\\d+")
        assert r.is_match("12345") is True
        assert r.is_match("abc") is False

    def test_invalid_pattern(self):
        with pytest.raises(ValueError, match="Invalid XSD regex"):
            XsdRegex("[invalid")

    def test_repr(self):
        r = XsdRegex("[a-z]+")
        assert repr(r) == "XsdRegex('[a-z]+')"

    def test_str(self):
        r = XsdRegex("[a-z]+")
        assert str(r) == "[a-z]+"


# ============================================================================
# Exceptions
# ============================================================================


class TestExceptions:
    def test_parse_error_is_exception(self):
        with pytest.raises(Exception):
            parse("<unclosed")

    def test_parse_error_type(self):
        with pytest.raises(XmlParseError):
            parse("<unclosed")

    def test_wellformedness_error(self):
        # Duplicate attributes trigger well-formedness error
        with pytest.raises((XmlWellFormednessError, XmlParseError)):
            parse('<root a="1" a="2"/>')

    def test_namespace_error(self):
        # Using an undeclared prefix triggers namespace error
        with pytest.raises((XmlNamespaceError, XmlParseError)):
            parse("<ns:root/>")

    def test_xpath_error(self):
        doc = parse("<root/>")
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        with pytest.raises(XPathError):
            xpath.evaluate(doc, "///[[[")

    def test_xsd_validation_error_exception(self):
        """XsdValidationError should be a valid exception class."""
        assert issubclass(XsdValidationError, Exception)


# ============================================================================
# Round-trip / integration tests
# ============================================================================


class TestIntegration:
    def test_parse_modify_serialize(self):
        doc = parse("<root><item>old</item></root>")
        items = doc.get_elements_by_tag_name("item")
        # Remove text child, add new text
        text_node = items[0].children[0]
        doc.remove_child(items[0], text_node)
        new_text = doc.create_text("new")
        doc.append_child(items[0], new_text)
        result = doc.to_xml()
        assert "<item>new</item>" in result

    def test_build_and_validate(self):
        schema = """\
<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="greeting" type="xs:string"/>
</xs:schema>
"""
        w = XmlWriter()
        w.start_element("greeting")
        w.text("Hello, World!")
        w.end_element("greeting")
        validator = XsdValidator(schema)
        assert validator.is_valid_str(w.to_string()) is True

    def test_xpath_on_modified_document(self):
        doc = parse("<root><a/></root>")
        b = doc.create_element("b")
        doc.append_child(doc.document_element, b)
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        nodes = xpath.select(doc, "/root/b")
        assert len(nodes) == 1

    def test_namespace_roundtrip(self):
        xml = '<root xmlns="urn:default" xmlns:ns="urn:ns"><ns:child/></root>'
        doc = parse(xml)
        el = doc.document_element
        assert el.tag.namespace_uri == "urn:default"
        children = el.children
        assert children[0].tag.namespace_uri == "urn:ns"
        assert children[0].tag.prefix == "ns"

    def test_complex_xpath_workflow(self):
        xml = """\
<bookstore>
  <book category="fiction">
    <title>The Great Gatsby</title>
    <price>10.99</price>
  </book>
  <book category="non-fiction">
    <title>A Brief History of Time</title>
    <price>15.99</price>
  </book>
</bookstore>
"""
        doc = parse(xml)
        doc.prepare_xpath()
        xpath = XPathEvaluator()

        # Select all books
        books = xpath.select(doc, "//book")
        assert len(books) == 2

        # Select fiction books
        fiction = xpath.select(doc, "//book[@category='fiction']")
        assert len(fiction) == 1

        # Get title text
        title = xpath.evaluate(doc, "string(//book[@category='fiction']/title)")
        assert title == "The Great Gatsby"

        # Count books
        count = xpath.evaluate(doc, "count(//book)")
        assert count == 2.0

    def test_xsd_complex_type(self):
        schema = """\
<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="person">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="name" type="xs:string"/>
        <xs:element name="email" type="xs:string" minOccurs="0"/>
        <xs:element name="age" type="xs:positiveInteger"/>
      </xs:sequence>
      <xs:attribute name="id" type="xs:integer" use="required"/>
    </xs:complexType>
  </xs:element>
</xs:schema>
"""
        validator = XsdValidator(schema)

        # Valid
        assert validator.is_valid_str(
            '<person id="1"><name>Alice</name><age>30</age></person>'
        )

        # Valid with optional email
        assert validator.is_valid_str(
            '<person id="1"><name>Alice</name><email>a@b.com</email><age>30</age></person>'
        )

        # Missing required attribute
        assert not validator.is_valid_str(
            "<person><name>Alice</name><age>30</age></person>"
        )

        # Invalid age (not positive integer)
        assert not validator.is_valid_str(
            '<person id="1"><name>Alice</name><age>-5</age></person>'
        )

    def test_detach_and_reattach(self):
        doc = parse("<root><a/><b/><c/></root>")
        root = doc.document_element
        b = root.children[1]
        doc.detach(b)
        assert len(root.children) == 2
        assert root.children[0].tag.local_name == "a"
        assert root.children[1].tag.local_name == "c"

        # Reattach at the beginning
        doc.insert_before(root, b, root.children[0])
        assert root.children[0].tag.local_name == "b"
        assert root.children[1].tag.local_name == "a"
        assert root.children[2].tag.local_name == "c"

    def test_writer_roundtrip(self):
        """Build with XmlWriter, parse back, query with XPath."""
        w = XmlWriter()
        w.write_declaration()
        w.start_element("catalog")
        for i in range(5):
            w.start_element("product", [("id", str(i))])
            w.text(f"Product {i}")
            w.end_element("product")
        w.end_element("catalog")

        doc = parse(w.to_string())
        doc.prepare_xpath()
        xpath = XPathEvaluator()
        products = xpath.select(doc, "//product")
        assert len(products) == 5
        assert xpath.evaluate(doc, "string(//product[@id='3'])") == "Product 3"


# ============================================================================
# Uppsala 0.3.0 new APIs
# ============================================================================


class TestElementText:
    """Tests for Node.element_text property."""

    def test_simple_element(self):
        doc = parse("<name>hello</name>")
        root = doc.document_element
        assert root.element_text == "hello"

    def test_empty_element(self):
        doc = parse("<name/>")
        root = doc.document_element
        assert root.element_text is None

    def test_nested_elements(self):
        doc = parse("<root><a>first</a><b>second</b></root>")
        root = doc.document_element
        # element_text only gets the first text/cdata child, not nested text
        assert root.element_text is None
        children = root.children
        assert children[0].element_text == "first"
        assert children[1].element_text == "second"

    def test_cdata_child(self):
        doc = parse("<name><![CDATA[hello]]></name>")
        root = doc.document_element
        assert root.element_text == "hello"

    def test_mixed_content(self):
        doc = parse("<p>text<b>bold</b></p>")
        root = doc.document_element
        # Gets only the first text child
        assert root.element_text == "text"

    def test_non_element_node(self):
        doc = parse("<root>text</root>")
        text_node = doc.document_element.children[0]
        assert text_node.element_text is None


class TestSourceTracking:
    """Tests for Node.source, Node.source_range, and Document.input_text."""

    def test_input_text(self):
        xml = "<root>hello</root>"
        doc = parse(xml)
        assert doc.input_text == xml

    def test_input_text_empty_doc(self):
        doc = Document.empty()
        assert doc.input_text == ""

    def test_source_range(self):
        xml = "<root><child>text</child></root>"
        doc = parse(xml)
        root = doc.document_element
        rng = root.source_range
        assert rng is not None
        start, end = rng
        assert xml[start:end] == xml

    def test_source(self):
        xml = "<root><child>text</child></root>"
        doc = parse(xml)
        child = doc.document_element.children[0]
        assert child.source == "<child>text</child>"

    def test_source_none_for_created_node(self):
        doc = Document.empty()
        elem = doc.create_element("foo")
        assert elem.source is None
        assert elem.source_range is None

    def test_source_with_attributes(self):
        xml = '<root><item id="1">hello</item></root>'
        doc = parse(xml)
        item = doc.document_element.children[0]
        assert item.source == '<item id="1">hello</item>'


class TestNamespaceSearch:
    """Tests for first_child_element_by_name_ns and child_elements_by_name_ns."""

    NS = "urn:example"

    def test_first_child_element_by_name_ns(self):
        xml = '<root xmlns:a="urn:example"><a:x>1</a:x><a:y>2</a:y><a:x>3</a:x></root>'
        doc = parse(xml)
        root = doc.document_element
        first_x = root.first_child_element_by_name_ns(self.NS, "x")
        assert first_x is not None
        assert first_x.element_text == "1"

    def test_first_child_element_by_name_ns_not_found(self):
        xml = '<root xmlns:a="urn:example"><a:x>1</a:x></root>'
        doc = parse(xml)
        root = doc.document_element
        result = root.first_child_element_by_name_ns(self.NS, "missing")
        assert result is None

    def test_child_elements_by_name_ns(self):
        xml = '<root xmlns:a="urn:example"><a:x>1</a:x><a:y>2</a:y><a:x>3</a:x></root>'
        doc = parse(xml)
        root = doc.document_element
        xs = root.child_elements_by_name_ns(self.NS, "x")
        assert len(xs) == 2
        assert xs[0].element_text == "1"
        assert xs[1].element_text == "3"

    def test_child_elements_by_name_ns_empty(self):
        xml = '<root xmlns:a="urn:example"><a:x>1</a:x></root>'
        doc = parse(xml)
        root = doc.document_element
        result = root.child_elements_by_name_ns(self.NS, "missing")
        assert result == []

    def test_only_direct_children(self):
        xml = '<root xmlns:a="urn:example"><wrapper><a:x>nested</a:x></wrapper></root>'
        doc = parse(xml)
        root = doc.document_element
        # Should not find nested elements, only direct children
        result = root.child_elements_by_name_ns(self.NS, "x")
        assert result == []


class TestMatchesNameNs:
    """Tests for Node.matches_name_ns."""

    def test_matches(self):
        xml = '<a:root xmlns:a="urn:example">text</a:root>'
        doc = parse(xml)
        root = doc.document_element
        assert root.matches_name_ns("urn:example", "root") is True

    def test_no_match_wrong_ns(self):
        xml = '<a:root xmlns:a="urn:example">text</a:root>'
        doc = parse(xml)
        root = doc.document_element
        assert root.matches_name_ns("urn:other", "root") is False

    def test_no_match_wrong_name(self):
        xml = '<a:root xmlns:a="urn:example">text</a:root>'
        doc = parse(xml)
        root = doc.document_element
        assert root.matches_name_ns("urn:example", "other") is False

    def test_non_element_returns_false(self):
        doc = parse("<root>text</root>")
        text_node = doc.document_element.children[0]
        assert text_node.matches_name_ns("", "root") is False


class TestQNameMatches:
    """Tests for QName.matches."""

    def test_matches_local_only(self):
        q = QName("root")
        assert q.matches("root") is True
        assert q.matches("other") is False

    def test_matches_with_namespace(self):
        q = QName("root", namespace_uri="urn:example")
        assert q.matches("root", namespace_uri="urn:example") is True
        assert q.matches("root", namespace_uri="urn:other") is False
        assert q.matches("root") is False

    def test_matches_no_ns_on_ns_qname(self):
        q = QName("root", namespace_uri="urn:example")
        # No namespace_uri means None, should not match a QName that has a namespace
        assert q.matches("root") is False

    def test_matches_no_ns_on_local_qname(self):
        q = QName("root")
        assert q.matches("root") is True
        assert q.matches("root", namespace_uri="urn:example") is False
