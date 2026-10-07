"""External XSLT parameters: type preservation, isolation, and lxml parity."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest
from pyuppsala import Document, Xslt, etree as P

STYLE = '''<xsl:stylesheet version="1.0"
 xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:p="urn:param" xmlns:q="urn:param">
 <xsl:output omit-xml-declaration="yes"/>
 <xsl:variable name="derived" select="concat($value, '!')"/>
 <xsl:param name="value" select="'default'"/>
 <xsl:param name="p:qualified" select="'fallback'"/>
 <xsl:template match="/"><out value="{$value}" derived="{$derived}" qualified="{$q:qualified}">
 <xsl:if test="$value"><yes/></xsl:if>
 </out></xsl:template></xsl:stylesheet>'''
SOURCE = '<root><item id="one"/><item id="two"/></root>'


def transform(style=STYLE):
    return P.XSLT(P.fromstring(style))


@pytest.mark.parametrize('value', ['', 'O\'Brien "quoted" & <xml> å漢字', "&apos;", 'https://example.org/?a=1&b=2'])
@pytest.mark.parametrize('fallback', [False, True])
def test_literals_and_paths(value, fallback):
    t = transform()
    source = P.fromstring(('<!--force serialized path-->' if fallback else '') + SOURCE)
    result = t(source, value=P.XSLT.strparam(value)).getroot()
    assert result.get('value') == value
    assert result.get('derived') == value + '!'
    assert t(source).getroot().get('value') == 'default'


@pytest.mark.parametrize('expr', ["'quoted'", '5', 'false()', 'true()', 'count(/root/item)', 'string(/root/item/@id)', 'position()', 'last()'])
def test_expression_parity(expr):
    L = pytest.importorskip('lxml.etree')
    ours = transform()(P.fromstring(SOURCE), value=expr).getroot()
    theirs = L.XSLT(L.fromstring(STYLE.encode()))(L.fromstring(SOURCE.encode()), value=expr).getroot()
    assert dict(ours.attrib) == dict(theirs.attrib)
    assert len(ours) == len(theirs)


def test_nodeset_stays_nodeset():
    style = STYLE.replace('<xsl:if test="$value"><yes/></xsl:if>', '<xsl:copy-of select="$value"/>')
    t = transform(style)
    for xml in [SOURCE, '<!--fallback-->' + SOURCE]:
        out = t(P.fromstring(xml), value='/root/item').getroot()
        assert [child.get('id') for child in out] == ['one', 'two']


@pytest.mark.parametrize('name', ['p:qualified', 'q:qualified', '{urn:param}qualified'])
def test_expanded_names(name):
    assert transform()(P.fromstring(SOURCE), **{name: P.XSLT.strparam('yes')}).getroot().get('qualified') == 'yes'


def test_errors_and_reuse():
    t = transform()
    source = P.fromstring(SOURCE)
    for params in [{'value': '('}, {'value': '$absent'}, {'bad:name': "'x'"}]:
        with pytest.raises(P.XSLTApplyError):
            t(source, **params)
        assert t.error_log
        assert t(source).getroot().get('value') == 'default'
        assert t.error_log == []
    for value in [None, 4, True, object()]:
        with pytest.raises(TypeError):
            t(source, value=value)
        with pytest.raises(TypeError):
            P.XSLT.strparam(value)


def test_variables_cannot_be_overridden_and_local_shadowing():
    t = transform()
    assert t(P.fromstring(SOURCE), derived=P.XSLT.strparam('wrong')).getroot().get('derived') == 'default!'
    style = STYLE.replace('<out value=', '<xsl:param name="value" select="\'local\'"/><out value=')
    assert transform(style)(P.fromstring(SOURCE), value=P.XSLT.strparam('external')).getroot().get('value') == 'local'


def test_concurrent_calls_and_compiled_xpath():
    t = transform()
    def run(i):
        return t(P.fromstring(SOURCE), value=P.XSLT.strparam(str(i))).getroot().get('value')
    with ThreadPoolExecutor(max_workers=4) as pool:
        assert list(pool.map(run, range(32))) == [str(i) for i in range(32)]
    assert t(P.fromstring(SOURCE), value=P.XPath('count(/root/item)')).getroot().get('value') == '2'


def test_native_paths_and_conflicting_parameters():
    t = Xslt(STYLE)
    doc = Document(SOURCE)
    for parameters in [{'parameters': {'value': 'false()'}}, {'string_parameters': {'value': 'literal'}}]:
        assert t.transform(SOURCE, **parameters) == t.transform_document(doc, **parameters)
    with pytest.raises(ValueError):
        t.transform(SOURCE, parameters={'value': '1'}, string_parameters={'value': 'one'})


def test_global_dependency_cycle_is_error():
    style = STYLE.replace("select=\"'default'\"", 'select="$derived"')
    with pytest.raises(P.XSLTApplyError, match='circular'):
        transform(style)(P.fromstring(SOURCE))
    assert transform(style)(P.fromstring(SOURCE), value=P.XSLT.strparam('override')).getroot().get('derived') == 'override!'


def test_pyff_publication_stylesheet():
    from datetime import datetime
    style = (Path(__file__).parent / 'fixtures' / 'pubinfo.xsl').read_bytes()
    t = P.XSLT(P.fromstring(style))
    value = 'https://example.org/O\'Brien?x="quoted"&y=ö'
    for content in ['', '<md:Extensions/>']:
        source = P.fromstring('<md:EntitiesDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata">' + content + '</md:EntitiesDescriptor>')
        result = t(source, publisher=P.XSLT.strparam(value)).getroot()
        publication = result.find('.//{urn:oasis:names:tc:SAML:metadata:rpi}PublicationInfo')
        assert publication.get('publisher') == value
        assert datetime.fromisoformat(publication.get('creationInstant').replace('Z', '+00:00')).tzinfo


def test_parameters_follow_keyword_order_like_lxml():
    L = pytest.importorskip('lxml.etree')
    for E in [P, L]:
        t = E.XSLT(E.fromstring(STYLE.encode()))
        source = E.fromstring(SOURCE.encode())
        result = t(source, **{'value': E.XSLT.strparam('first'), 'p:qualified': "concat($value, '-second')"})
        assert result.getroot().get('qualified') == 'first-second'
        with pytest.raises(E.XSLTApplyError):
            t(source, **{'p:qualified': '$value', 'value': "'late'"})
        result = t(source, **{'p:qualified': '$value', 'value': E.XSLT.strparam('late')})
        assert result.getroot().get('qualified') == 'late'



def test_native_ordered_literals_and_expressions():
    t = Xslt(STYLE)
    params = {'value': Xslt.strparam('literal'), 'p:qualified': "concat($value, '!')"}
    root = P.fromstring(t.transform(SOURCE, parameters=params))
    assert root.get('qualified') == 'literal!'
    assert t.transform(SOURCE, parameters=params) == t.transform_document(Document(SOURCE), parameters=params)
    root = P.fromstring(t.transform(SOURCE, string_parameters={'value': 'literal'}, parameters={'p:qualified': '$value'}))
    assert root.get('qualified') == 'literal'


def test_bad_unused_expression_is_reported():
    t = transform()
    with pytest.raises(P.XSLTApplyError):
        t(P.fromstring(SOURCE), unused='(')


@pytest.mark.parametrize('stylesheet', ['tidy.xsl', 'eidas-cleanup.xsl'])
@pytest.mark.parametrize('fallback', [False, True])
def test_pyff_cleanup_removes_xml_attributes(stylesheet, fallback):
    """Keep the real cleanup stylesheets working through both Python paths."""
    L = pytest.importorskip('lxml.etree')
    style = (Path(__file__).parent / 'fixtures' / stylesheet).read_bytes()
    source = b'''<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata"
      entityID="https://example.org" ID="old" xml:id="oldxml"
      xml:base="https://example.org/" xml:lang="en" keep="yes">
      <md:Extensions/><md:IDPSSODescriptor xml:id="nested"/>
    </md:EntityDescriptor>'''
    if fallback:
        source = b'<!--force serialized input-->' + source
    actual = P.XSLT(P.fromstring(style))(P.fromstring(source)).getroot()
    expected = L.XSLT(L.fromstring(style))(L.fromstring(source)).getroot()
    def tree(node):
        return node.tag, dict(node.attrib), node.text, [tree(c) for c in node]
    assert tree(actual) == tree(expected)
    assert actual.get('{http://www.w3.org/XML/1998/namespace}lang') == 'en'
    assert actual.get('keep') == 'yes'
    for node in actual.iter():
        assert node.get('{http://www.w3.org/XML/1998/namespace}id') is None
        assert node.get('{http://www.w3.org/XML/1998/namespace}base') is None


def test_xpath_cannot_rebind_xml_prefix():
    root = P.fromstring('<root xmlns:other="urn:other" xml:id="real" other:id="wrong"/>')
    assert root.xpath('string(@xml:id)', namespaces={'xml': 'urn:other'}) == 'real'
