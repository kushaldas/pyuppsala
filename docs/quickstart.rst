Quick start
===========

Installation
------------

Install from PyPI::

    pip install pyuppsala

Or with `uv <https://docs.astral.sh/uv/>`_::

    uv add pyuppsala

Requirements: Python 3.10 or later. No C compiler needed -- the package ships
pre-built wheels compiled from Rust.

Parse an XML document
---------------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<root><child>hello</child></root>")
    el = doc.document_element
    print(el.tag.local_name)      # "root"
    print(el.text_content)        # "hello"

You can also parse from bytes (UTF-8 and UTF-16 are auto-detected):

.. code-block:: python

    from pyuppsala import parse_bytes

    doc = parse_bytes(b"<root>ok</root>")

Query with XPath
----------------

.. code-block:: python

    from pyuppsala import Document, XPathEvaluator

    doc = Document("""\
    <bookstore>
      <book category="fiction">
        <title>The Great Gatsby</title>
      </book>
      <book category="non-fiction">
        <title>A Brief History of Time</title>
      </book>
    </bookstore>
    """)
    doc.prepare_xpath()

    xpath = XPathEvaluator()

    # Select nodes
    books = xpath.select(doc, "//book")
    print(len(books))  # 2

    # Evaluate to a string
    title = xpath.evaluate(doc, "string(//book[@category='fiction']/title)")
    print(title)  # "The Great Gatsby"

    # Evaluate to a number
    count = xpath.evaluate(doc, "count(//book)")
    print(count)  # 2.0

    # Evaluate to a boolean
    has_fiction = xpath.evaluate(doc, "boolean(//book[@category='fiction'])")
    print(has_fiction)  # True

Namespace-aware XPath requires registering prefixes:

.. code-block:: python

    doc = Document('<root xmlns:ns="urn:test"><ns:item/></root>')
    doc.prepare_xpath()

    xpath = XPathEvaluator()
    xpath.add_namespace("ns", "urn:test")
    nodes = xpath.select(doc, "/root/ns:item")

Validate with XSD
-----------------

.. code-block:: python

    from pyuppsala import XsdValidator

    schema = """\
    <?xml version="1.0"?>
    <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="greeting" type="xs:string"/>
    </xs:schema>
    """

    validator = XsdValidator(schema)

    # Quick boolean check
    print(validator.is_valid_str("<greeting>Hello</greeting>"))  # True

    # Detailed error list
    errors = validator.validate_str("<greeting><bad/></greeting>")
    for err in errors:
        print(err)  # prints line:column: message

Build XML without a DOM
-----------------------

.. code-block:: python

    from pyuppsala import XmlWriter

    w = XmlWriter()
    w.write_declaration()
    w.start_element("catalog", [("xmlns", "urn:example")])
    w.start_element("item", [("id", "1")])
    w.text("Widget")
    w.end_element("item")
    w.end_element("catalog")
    print(w.to_string())

Mutate the DOM
--------------

.. code-block:: python

    from pyuppsala import Document

    doc = Document("<root><a/></root>")
    root = doc.document_element

    # Create and attach new nodes
    b = doc.create_element("b")
    doc.append_child(root, b)

    text = doc.create_text("hello")
    doc.append_child(b, text)

    # Detach and reattach
    doc.detach(b)
    doc.insert_before(root, b, root.children[0])

    print(doc.to_xml())
