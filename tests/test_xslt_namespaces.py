"""Copied XSLT attributes must retain their namespace (issue #36)."""

import xml.etree.ElementTree as ET

import pytest

from pyuppsala import Document, Xslt, etree

SOURCE = '<root xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><v xsi:type="string">å漢字</v></root>'
URI = "http://www.w3.org/2001/XMLSchema-instance"


@pytest.mark.parametrize("copy_snippet", [
    '<xsl:copy><xsl:apply-templates select="@*|node()"/></xsl:copy>',
    '<v><xsl:copy-of select="@*|node()"/></v>',
    '<xsl:copy-of select="."/>',
])
@pytest.mark.parametrize("api", ["etree", "string", "document"])
@pytest.mark.parametrize("omit_declaration", ["yes", "no"])
def test_copied_attribute_namespace(copy_snippet, api, omit_declaration):
    style = f'''<xsl:stylesheet version="1.0"
        xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:output encoding="UTF-8" omit-xml-declaration="{omit_declaration}"/>
      <xsl:template match="/root"><root><xsl:apply-templates/></root></xsl:template>
      <xsl:template match="v">{copy_snippet}</xsl:template>
      <xsl:template match="@*|text()"><xsl:copy/></xsl:template>
    </xsl:stylesheet>'''
    if api == "etree":
        result = etree.XSLT(etree.fromstring(style))(etree.fromstring(SOURCE))
        assert result.getroot()[0].get(f"{{{URI}}}type") == "string"
        output = str(result)
    else:
        transform = Xslt(style)
        output = (transform.transform(SOURCE) if api == "string"
                  else transform.transform_document(Document(SOURCE)))
    assert output.startswith("<?xml") == (omit_declaration == "no")
    output_bytes = output.encode("utf-8") if isinstance(output, str) else output
    child = ET.fromstring(output_bytes)[0]
    assert child.attrib == {f"{{{URI}}}type": "string"}
    assert child.text == "å漢字"
