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
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    root = E.fromstring(xml)
    node = root if tag is None else root.find(tag)
    before = E.tostring(root)
    parent = node.getparent()
    reference = schema._validator.validate_str(E.tostring(node, encoding='unicode'))
    assert schema.validate(node) is valid
    assert [e.message for e in schema.error_log] == [e.message for e in reference]
    assert schema.validate(E.ElementTree(node)) is valid
    assert E.tostring(root) == before
    assert node.getparent() is parent


def test_native_validation_observes_mutation_and_detached_elements():
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    root = E.fromstring('<outer><number>42</number></outer>')
    node = root[0]
    assert schema.validate(node)
    root.remove(node)
    node.text = 'bad'
    assert not schema.validate(node)
    with pytest.raises(E.DocumentInvalid) as error:
        schema.assertValid(node)
    assert error.value.error_log
    node.text = '7'
    assert schema.validate(node)
    assert schema.error_log == []


def test_native_validation_releases_gil_with_shared_document():
    validator = pyuppsala.XsdValidator(SCHEMA)
    doc = pyuppsala.parse('<outer><number>42</number><number>bad</number></outer>')
    nodes = doc.get_elements_by_tag_name('number')
    with ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(lambda i: not validator.validate_node(nodes[i % 2]), range(40)))
    assert results == [i % 2 == 0 for i in range(40)]


def test_native_validation_rejects_non_element():
    validator = pyuppsala.XsdValidator(SCHEMA)
    with pytest.raises(RuntimeError, match='element node'):
        validator.validate_node(pyuppsala.parse('<number>1</number>').root)


def test_identity_constraints_are_scoped_to_the_validated_subtree():
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
    assert schema.validate(outer[0])
    assert not schema.validate(outer[1])
    assert schema.validate(outer[0])
    assert schema.error_log == []


def test_document_root_validation_observes_mutation():
    schema = E.XMLSchema(E.fromstring(SCHEMA))
    node = E.fromstring('<number>42</number>')
    assert schema.validate(node)
    node.text = 'bad'
    assert not schema.validate(node)
    assert schema.error_log
    node.text = '7'
    assert schema.validate(node)
    assert schema.error_log == []
