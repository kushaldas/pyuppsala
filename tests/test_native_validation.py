"""Native XSD validation must agree with standalone serialization."""
from concurrent.futures import ThreadPoolExecutor

import pytest
import pyuppsala
from pyuppsala import etree as E

SCHEMA = '''<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="value" type="xs:anyType"/>
  <xs:element name="number" type="xs:int"/>
  <xs:element name="qname" type="xs:QName"/>
</xs:schema>'''

@pytest.mark.parametrize('xml,tag,valid', [
    ('<number>42</number>', None, True),
    ('<number>bad</number>', None, False),
    ('<unknown><number>42</number><number>bad</number></unknown>', 'number', True),
    ('<outer xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><value xsi:type="xs:int">42</value></outer>', 'value', True),
    ('<outer xmlns:xs="urn:wrong"><inner xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><value xsi:type="xs:int">bad</value></inner></outer>', './/value', False),
    ('<outer xmlns:p="urn:outer"><inner xmlns:p="urn:inner"><qname>p:x</qname></inner></outer>', './/qname', True),
    ('<outer xmlns="urn:outer"><inner xmlns=""><number>42</number></inner></outer>', './/number', True),
])
def test_native_validation_matches_serialized(xml, tag, valid):
    """Match serialized validation without changing ancestry or namespace context."""
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    root = E.fromstring(xml)
    node = root if tag is None else root.find(tag)
    before = E.tostring(root)
    parent = node.getparent()
    # Standalone serialization supplies the reference behavior for inherited
    # namespaces, including shadowed prefixes and a reset default namespace.
    reference = schema._validator.validate_str(E.tostring(node, encoding='unicode'))
    assert schema.experimental_validate(node) is valid
    assert [e.message for e in schema.error_log] == [e.message for e in reference]
    assert schema.experimental_validate(E.ElementTree(node)) is valid
    assert E.tostring(root) == before
    assert node.getparent() is parent


def test_native_validation_observes_mutation_and_detached_elements():
    """Validate detached nodes using their current text and clear stale errors."""
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    root = E.fromstring('<outer><number>42</number></outer>')
    node = root[0]
    assert schema.experimental_validate(node)
    root.remove(node)
    node.text = 'bad'
    assert not schema.experimental_validate(node)
    with pytest.raises(E.DocumentInvalid) as error:
        schema.experimental_assertValid(node)
    assert error.value.error_log
    node.text = '7'
    assert schema.experimental_validate(node)
    assert schema.error_log == []


def test_native_validation_releases_gil_with_shared_document():
    """Check concurrent validation results for nodes sharing one document."""
    # This exercises concurrent callers; it does not measure GIL release or
    # prove that the native validation calls execute in parallel.
    validator = pyuppsala.XsdValidator(SCHEMA)
    doc = pyuppsala.parse('<outer><number>42</number><number>bad</number></outer>')
    nodes = doc.get_elements_by_tag_name('number')
    with ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(lambda i: not validator.experimental_validate_node(nodes[i % 2]), range(40)))
    assert results == [i % 2 == 0 for i in range(40)]


def test_native_validation_rejects_non_element():
    """Reject the document node rather than treating it as the root element."""
    validator = pyuppsala.XsdValidator(SCHEMA)
    with pytest.raises(RuntimeError, match='element node'):
        validator.experimental_validate_node(pyuppsala.parse('<number>1</number>').root)


def test_identity_constraints_are_scoped_to_the_validated_subtree():
    """Keep uniqueness checks and their errors local to each validated group."""
    schema = E.XMLSchema(E.fromstring('''<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="group"><xs:complexType><xs:sequence>
        <xs:element name="item" minOccurs="0" maxOccurs="unbounded"><xs:complexType>
          <xs:attribute name="key" type="xs:string"/>
        </xs:complexType></xs:element>
      </xs:sequence></xs:complexType>
      <xs:unique name="uniqueKey"><xs:selector xpath="item"/><xs:field xpath="@key"/></xs:unique>
      </xs:element></xs:schema>'''))
    outer = E.fromstring('<outer><group><item key="a"/></group>'
                         '<group><item key="a"/><item key="a"/></group></outer>')
    # A duplicate in the sibling group must not invalidate the first group.
    assert schema.experimental_validate(outer[0])
    assert not schema.experimental_validate(outer[1])
    assert schema.experimental_validate(outer[0])
    assert schema.error_log == []


def test_document_root_validation_observes_mutation():
    """Have the whole-document path observe edits and reset validation errors."""
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    node = E.fromstring('<number>42</number>')
    assert schema.experimental_validate(node)
    node.text = 'bad'
    assert not schema.experimental_validate(node)
    assert schema.error_log
    node.text = '7'
    assert schema.experimental_validate(node)
    assert schema.error_log == []



def test_standard_validation_does_not_use_experimental_api():
    """Keep all three stable validation entry points on the serialized API."""
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    calls = []

    class StableValidator:
        """Expose only the stable method so experimental dispatch would fail."""

        def validate_str(self, xml):
            """Record serialized inputs and report successful validation."""
            calls.append(xml)
            return []

    schema._validator = StableValidator()
    node = E.fromstring('<number>42</number>')
    assert schema.validate(node)
    schema.assertValid(node)
    assert schema(node)
    assert calls == ['<number>42</number>'] * 3


@pytest.mark.parametrize('subtree', [False, True])
@pytest.mark.parametrize('reset_default', [False, True])
def test_native_validation_qualifies_bare_names_without_mutation(subtree, reset_default):
    """Match lexical default namespaces for roots, subtrees, and namespace resets."""
    schema = E.XMLSchema(E.fromstring('''
      <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                 targetNamespace="urn:kml" elementFormDefault="qualified">
        <xs:element name="kml"><xs:complexType><xs:sequence>
          <xs:element name="child" form="%s"/>
        </xs:sequence></xs:complexType></xs:element>
      </xs:schema>''' % ('unqualified' if reset_default else 'qualified')))
    outer = E.Element('outer', nsmap={None: 'urn:kml'})
    node = E.SubElement(outer, 'kml') if subtree else E.Element('kml', nsmap={None: 'urn:kml'})
    child = E.SubElement(node, 'child', nsmap={None: ''} if reset_default else None)
    before = E.tostring(outer if subtree else node)
    names = (node.tag, child.tag)
    parent = node.getparent()
    assert schema.validate(node)
    assert schema.experimental_validate(node)
    assert schema.experimental_validate(E.ElementTree(node))
    assert (node.tag, child.tag) == names
    assert node.getparent() is parent
    assert E.tostring(outer if subtree else node) == before


@pytest.mark.parametrize('method', ['experimental_validate', 'experimental_assertValid'])
def test_native_validation_empty_tree_reports_missing_root(method):
    """Give empty trees the stable API's explicit missing-root assertion."""
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    with pytest.raises(AssertionError, match='ElementTree not initialized, missing root'):
        getattr(schema, method)(E.ElementTree())
