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

Fast text extraction with element_text
--------------------------------------

Use :attr:`~Node.element_text` for simple ``<tag>value</tag>`` patterns.
It returns the text of the first Text or CDATA child without recursion:

.. code-block:: python

    from pyuppsala import Document

    doc = Document("""\
    <catalog>
      <product>
        <name>Widget</name>
        <price>9.99</price>
        <description>A <b>great</b> product</description>
      </product>
    </catalog>
    """)

    product = doc.get_elements_by_tag_name("product")[0]
    for child in product:
        if child.kind == "element":
            # element_text is fast: only looks at the first text child
            print(f"{child.tag.local_name}: {child.element_text}")
    # name: Widget
    # price: 9.99
    # description: A   <-- only the first text child "A ", not "A great product"

    # For recursive text content, use text_content instead
    desc = doc.get_elements_by_tag_name("description")[0]
    print(desc.text_content)  # "A great product"

Source tracking for error reporting
-----------------------------------

Use :attr:`~Node.source`, :attr:`~Node.source_range`, and
:attr:`Document.input_text` for source-level error reporting:

.. code-block:: python

    from pyuppsala import Document

    xml = """\
    <config>
      <database host="localhost" port="5432"/>
      <cache host="redis.local" port="abc"/>
    </config>"""

    doc = Document(xml)

    for db in doc.get_elements_by_tag_name("cache"):
        port = db.get_attribute("port")
        if not port.isdigit():
            print(f"Invalid port at line {db.line}, column {db.column}")
            print(f"Source: {db.source}")
            # Invalid port at line 3, column 3
            # Source: <cache host="redis.local" port="abc"/>

Byte ranges are useful for highlighting in editors:

.. code-block:: python

    xml = '<root><item id="1">hello</item><item id="2">world</item></root>'
    doc = Document(xml)

    for item in doc.get_elements_by_tag_name("item"):
        start, end = item.source_range
        print(f"Bytes {start}-{end}: {xml[start:end]}")
    # Bytes 6-30: <item id="1">hello</item>
    # Bytes 30-54: <item id="2">world</item>

Namespace-aware child search
-----------------------------

Use :meth:`~Node.first_child_element_by_name_ns` and
:meth:`~Node.child_elements_by_name_ns` for efficient direct-child lookups
by namespace:

.. code-block:: python

    from pyuppsala import Document

    SAML_NS = "urn:oasis:names:tc:SAML:2.0:assertion"
    xml = f"""\
    <saml:Assertion xmlns:saml="{SAML_NS}">
      <saml:Issuer>https://idp.example.com</saml:Issuer>
      <saml:Subject>
        <saml:NameID>user@example.com</saml:NameID>
      </saml:Subject>
      <saml:Conditions NotBefore="2026-01-01T00:00:00Z"/>
      <saml:AttributeStatement>
        <saml:Attribute Name="email">
          <saml:AttributeValue>user@example.com</saml:AttributeValue>
        </saml:Attribute>
        <saml:Attribute Name="role">
          <saml:AttributeValue>admin</saml:AttributeValue>
        </saml:Attribute>
      </saml:AttributeStatement>
    </saml:Assertion>
    """

    doc = Document(xml)
    assertion = doc.document_element

    # Get the Issuer (first matching child)
    issuer = assertion.first_child_element_by_name_ns(SAML_NS, "Issuer")
    print(issuer.element_text)  # "https://idp.example.com"

    # Get Subject's NameID
    subject = assertion.first_child_element_by_name_ns(SAML_NS, "Subject")
    name_id = subject.first_child_element_by_name_ns(SAML_NS, "NameID")
    print(name_id.element_text)  # "user@example.com"

    # Get all Attribute elements from AttributeStatement
    attr_stmt = assertion.first_child_element_by_name_ns(SAML_NS, "AttributeStatement")
    attrs = attr_stmt.child_elements_by_name_ns(SAML_NS, "Attribute")
    for attr in attrs:
        name = attr.get_attribute("Name")
        value_elem = attr.first_child_element_by_name_ns(SAML_NS, "AttributeValue")
        print(f"{name} = {value_elem.element_text}")
    # email = user@example.com
    # role = admin

Element name matching
---------------------

Use :meth:`~Node.matches_name_ns` and :meth:`QName.matches` for
namespace-aware dispatch:

.. code-block:: python

    from pyuppsala import Document

    NS_A = "urn:app:config"
    NS_B = "urn:app:runtime"

    xml = f"""\
    <root xmlns:cfg="{NS_A}" xmlns:rt="{NS_B}">
      <cfg:setting>value1</cfg:setting>
      <rt:setting>value2</rt:setting>
      <cfg:setting>value3</cfg:setting>
    </root>
    """

    doc = Document(xml)
    root = doc.document_element

    for child in root:
        if child.kind != "element":
            continue
        if child.matches_name_ns(NS_A, "setting"):
            print(f"Config: {child.element_text}")
        elif child.matches_name_ns(NS_B, "setting"):
            print(f"Runtime: {child.element_text}")
    # Config: value1
    # Runtime: value2
    # Config: value3

QName.matches works on standalone QName objects too:

.. code-block:: python

    from pyuppsala import QName

    # Build a dispatch table
    handlers = {
        ("urn:app", "create"): lambda: "Creating...",
        ("urn:app", "delete"): lambda: "Deleting...",
    }

    q = QName("create", namespace_uri="urn:app", prefix="app")
    for (ns, name), handler in handlers.items():
        if q.matches(name, namespace_uri=ns):
            print(handler())  # "Creating..."

Building a document from scratch
---------------------------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document.empty()
    root = doc.create_element("catalog")
    doc.append_child(doc.root, root)

    products = [
        {"id": "1", "name": "Widget", "price": "9.99"},
        {"id": "2", "name": "Gadget", "price": "19.99"},
        {"id": "3", "name": "Doohickey", "price": "4.99"},
    ]

    for prod in products:
        elem = doc.create_element("product")
        elem.set_attribute("id", prod["id"])

        name = doc.create_element("name")
        doc.append_child(name, doc.create_text(prod["name"]))
        doc.append_child(elem, name)

        price = doc.create_element("price")
        doc.append_child(price, doc.create_text(prod["price"]))
        doc.append_child(elem, price)

        doc.append_child(root, elem)

    print(doc.to_xml_with_options(indent="  "))
    # <catalog>
    #   <product id="1">
    #     <name>Widget</name>
    #     <price>9.99</price>
    #   </product>
    #   <product id="2">
    #     <name>Gadget</name>
    #     <price>19.99</price>
    #   </product>
    #   <product id="3">
    #     <name>Doohickey</name>
    #     <price>4.99</price>
    #   </product>
    # </catalog>

XPath context nodes
-------------------

Use the ``context`` parameter to evaluate XPath relative to a specific node:

.. code-block:: python

    from pyuppsala import Document, XPathEvaluator

    doc = Document("""\
    <company>
      <dept name="Engineering">
        <employee>Alice</employee>
        <employee>Bob</employee>
      </dept>
      <dept name="Sales">
        <employee>Charlie</employee>
      </dept>
    </company>
    """)
    doc.prepare_xpath()

    xpath = XPathEvaluator()

    # Count all employees
    total = xpath.evaluate(doc, "count(//employee)")
    print(f"Total employees: {total}")  # 3.0

    # Count per department using context
    for dept in xpath.select(doc, "//dept"):
        name = dept.get_attribute("name")
        count = xpath.evaluate(doc, "count(employee)", context=dept)
        print(f"  {name}: {count}")
    # Engineering: 2.0
    # Sales: 1.0

Combining XmlWriter with validation
-------------------------------------

.. code-block:: python

    from pyuppsala import XmlWriter, XsdValidator, parse

    # Define a schema
    schema = """\
    <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="order">
        <xs:complexType>
          <xs:sequence>
            <xs:element name="item" maxOccurs="unbounded">
              <xs:complexType>
                <xs:simpleContent>
                  <xs:extension base="xs:string">
                    <xs:attribute name="qty" type="xs:positiveInteger" use="required"/>
                  </xs:extension>
                </xs:simpleContent>
              </xs:complexType>
            </xs:element>
          </xs:sequence>
          <xs:attribute name="id" type="xs:positiveInteger" use="required"/>
        </xs:complexType>
      </xs:element>
    </xs:schema>
    """
    validator = XsdValidator(schema)

    # Build XML with XmlWriter
    w = XmlWriter()
    w.start_element("order", [("id", "42")])
    w.start_element("item", [("qty", "3")])
    w.text("Widget")
    w.end_element("item")
    w.start_element("item", [("qty", "1")])
    w.text("Gadget")
    w.end_element("item")
    w.end_element("order")

    # Validate the generated XML
    xml = w.to_string()
    print(f"Valid: {validator.is_valid_str(xml)}")  # True

    # Parse and inspect
    doc = parse(xml)
    for item in doc.get_elements_by_tag_name("item"):
        print(f"  {item.element_text} x{item.get_attribute('qty')}")
    # Widget x3
    # Gadget x1

Tree manipulation: reorder children
-------------------------------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<list><c>3</c><a>1</a><b>2</b></list>")
    root = doc.document_element

    # Collect children and sort by tag name
    children = list(root.children)
    children.sort(key=lambda n: n.tag.local_name)

    # Detach all, then re-attach in order
    for child in children:
        doc.detach(child)
    for child in children:
        doc.append_child(root, child)

    print(doc.to_xml())
    # "<list><a>1</a><b>2</b><c>3</c></list>"

Node iteration patterns
-----------------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<root><a/><b/><c/><d/><e/></root>")
    root = doc.document_element

    # len() and indexing
    print(len(root))  # 5
    print(root[0].tag.local_name)   # "a"
    print(root[-1].tag.local_name)  # "e"

    # Iteration
    names = [child.tag.local_name for child in root]
    print(names)  # ['a', 'b', 'c', 'd', 'e']

    # Truthiness -- nodes are always truthy
    if root:
        print("Root exists")

    # String conversion
    print(str(root))   # "<root><a/><b/><c/><d/><e/></root>"
    print(repr(root))  # "Node(<root>)"
