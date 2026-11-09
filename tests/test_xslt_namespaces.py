"""Copied XSLT attributes must retain their namespace (issue #36)."""

import xml.etree.ElementTree as ET

import pytest

from pyuppsala import Document, Xslt, etree

SOURCE = '<root xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><v xsi:type="string">1</v></root>'
URI = "http://www.w3.org/2001/XMLSchema-instance"


@pytest.mark.parametrize("copy", [
    '<xsl:copy><xsl:apply-templates select="@*|node()"/></xsl:copy>',
    '<v><xsl:copy-of select="@*|node()"/></v>',
    '<xsl:copy-of select="."/>',
])
@pytest.mark.parametrize("api", ["etree", "string", "document"])
def test_copied_attribute_namespace(copy, api):
    style = f'''<xsl:stylesheet version="1.0"
        xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:template match="/root"><root><xsl:apply-templates/></root></xsl:template>
      <xsl:template match="v">{copy}</xsl:template>
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
    child = ET.fromstring(output)[0]
    assert child.attrib == {f"{{{URI}}}type": "string"}
    assert child.text == "1"
