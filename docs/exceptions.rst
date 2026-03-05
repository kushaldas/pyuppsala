Exceptions
==========

All pyuppsala exceptions inherit from Python's built-in :class:`Exception`.

.. exception:: XmlParseError

   Raised when the XML input is syntactically malformed (e.g. unclosed tags,
   invalid characters). The error message includes line and column numbers.

   .. code-block:: python

      from pyuppsala import parse, XmlParseError

      try:
          parse("<unclosed")
      except XmlParseError as e:
          print(e)  # "1:10: ..."

.. exception:: XmlWellFormednessError

   Raised when the XML violates a well-formedness constraint defined by
   XML 1.0, such as duplicate attributes or mismatched tags.

   .. code-block:: python

      from pyuppsala import parse, XmlWellFormednessError

      try:
          parse('<root a="1" a="2"/>')
      except XmlWellFormednessError as e:
          print(e)

.. exception:: XmlNamespaceError

   Raised when namespace processing fails, such as using an undeclared
   prefix.

   .. code-block:: python

      from pyuppsala import parse, XmlNamespaceError

      try:
          parse("<ns:root/>")  # "ns" is not declared
      except XmlNamespaceError as e:
          print(e)

.. exception:: XPathError

   Raised when an XPath expression cannot be parsed or evaluated.

   .. code-block:: python

      from pyuppsala import Document, XPathEvaluator, XPathError

      doc = Document("<root/>")
      doc.prepare_xpath()
      xpath = XPathEvaluator()

      try:
          xpath.evaluate(doc, "///invalid[")
      except XPathError as e:
          print(e)

.. exception:: XsdValidationError

   Raised when an XSD schema itself is invalid (not when an instance document
   fails validation -- that returns :class:`ValidationError` objects).

Exception hierarchy
-------------------

::

    Exception
    ├── XmlParseError
    ├── XmlWellFormednessError
    ├── XmlNamespaceError
    ├── XPathError
    └── XsdValidationError
