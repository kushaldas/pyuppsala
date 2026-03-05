Examples
========

Parse, query, modify
--------------------

Parse an HTML-like structure, find elements by tag name, and modify them:

.. code-block:: python

    from pyuppsala import Document

    xml = """\
    <html>
      <body>
        <p class="intro">Hello</p>
        <p class="body">World</p>
      </body>
    </html>
    """

    doc = Document(xml)

    # Find all <p> elements
    paragraphs = doc.get_elements_by_tag_name("p")
    for p in paragraphs:
        print(f"{p.get_attribute('class')}: {p.text_content}")

    # Add a new paragraph
    body = doc.get_elements_by_tag_name("body")[0]
    new_p = doc.create_element("p")
    new_p.set_attribute("class", "footer")
    text = doc.create_text("Goodbye")
    doc.append_child(new_p, text)
    doc.append_child(body, new_p)

    print(doc.to_xml_with_options(indent="  "))

Namespace-aware processing
--------------------------

.. code-block:: python

    from pyuppsala import Document, XPathEvaluator

    xml = """\
    <soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
      <soap:Body>
        <m:GetPrice xmlns:m="urn:example">
          <m:Item>Widget</m:Item>
        </m:GetPrice>
      </soap:Body>
    </soap:Envelope>
    """

    doc = Document(xml)
    doc.prepare_xpath()

    xpath = XPathEvaluator()
    xpath.add_namespace("soap", "http://schemas.xmlsoap.org/soap/envelope/")
    xpath.add_namespace("m", "urn:example")

    item = xpath.evaluate(doc, "string(//m:Item)")
    print(item)  # "Widget"

    body = xpath.select(doc, "/soap:Envelope/soap:Body")
    print(body[0].tag.prefixed_name)  # "soap:Body"

Schema validation with detailed errors
---------------------------------------

.. code-block:: python

    from pyuppsala import XsdValidator

    schema = """\
    <?xml version="1.0"?>
    <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="person">
        <xs:complexType>
          <xs:sequence>
            <xs:element name="name" type="xs:string"/>
            <xs:element name="age" type="xs:positiveInteger"/>
          </xs:sequence>
          <xs:attribute name="id" type="xs:integer" use="required"/>
        </xs:complexType>
      </xs:element>
    </xs:schema>
    """

    validator = XsdValidator(schema)

    # Valid document
    errors = validator.validate_str(
        '<person id="1"><name>Alice</name><age>30</age></person>'
    )
    assert len(errors) == 0

    # Invalid: missing required attribute, wrong element order
    errors = validator.validate_str(
        "<person><age>-5</age><name>Bob</name></person>"
    )
    for err in errors:
        print(f"  Line {err.line}, Col {err.column}: {err.message}")

Building XML with XmlWriter
----------------------------

.. code-block:: python

    from pyuppsala import XmlWriter, parse

    w = XmlWriter()
    w.write_declaration()
    w.start_element("feed", [("xmlns", "http://www.w3.org/2005/Atom")])

    for i, title in enumerate(["First Post", "Second Post"]):
        w.start_element("entry")
        w.start_element("title")
        w.text(title)
        w.end_element("title")
        w.start_element("id")
        w.text(f"urn:uuid:{i}")
        w.end_element("id")
        w.end_element("entry")

    w.end_element("feed")

    # Verify the output parses correctly
    doc = parse(w.to_string())
    entries = doc.get_elements_by_tag_name("entry")
    print(f"Generated {len(entries)} entries")  # 2

XSD regex patterns
------------------

.. code-block:: python

    from pyuppsala import XsdRegex

    # US ZIP code
    zip_re = XsdRegex(r"[0-9]{5}(-[0-9]{4})?")
    print(zip_re.is_match("12345"))       # True
    print(zip_re.is_match("12345-6789"))  # True
    print(zip_re.is_match("abcde"))       # False

    # Unicode letter categories
    letters = XsdRegex(r"\p{L}+")
    print(letters.is_match("Hello"))   # True
    print(letters.is_match("12345"))   # False

    # Character class subtraction (vowels removed)
    consonants = XsdRegex(r"[a-z-[aeiou]]+")
    print(consonants.is_match("bcdfg"))  # True
    print(consonants.is_match("abcde"))  # False

Streaming serialization
-----------------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<root><a/><b/><c/></root>")

    # Write to a file
    doc.write_to_file("/tmp/output.xml")

    # Pretty-print to a file
    pretty = doc.to_xml_with_options(indent="    ")
    with open("/tmp/pretty.xml", "w") as f:
        f.write(pretty)

    # Serialize a subtree
    a_node = doc.get_elements_by_tag_name("a")[0]
    print(a_node.to_xml())  # "<a/>"
