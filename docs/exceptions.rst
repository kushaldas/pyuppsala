Exceptions
==========

All pyuppsala exceptions inherit from Python's built-in :class:`Exception`.

.. exception:: XmlParseError

   Raised when the XML input is syntactically malformed (e.g. unclosed tags,
   invalid characters, unexpected end of input). The error message includes
   line and column numbers.

   .. code-block:: python

      from pyuppsala import parse, XmlParseError

      try:
          parse("<unclosed")
      except XmlParseError as e:
          print(e)  # "1:10: ..."

      try:
          parse("")
      except XmlParseError as e:
          print(e)  # "Unexpected end of input"

.. exception:: XmlWellFormednessError

   Raised when the XML violates a well-formedness constraint defined by
   XML 1.0, such as duplicate attributes or mismatched tags.

   .. code-block:: python

      from pyuppsala import parse, XmlWellFormednessError

      try:
          parse('<root a="1" a="2"/>')
      except XmlWellFormednessError as e:
          print(e)  # duplicate attribute error

      try:
          parse("<open></close>")
      except XmlWellFormednessError as e:
          print(e)  # mismatched end tag

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

   .. code-block:: python

      from pyuppsala import XsdValidator, XsdValidationError

      try:
          XsdValidator("<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'><xs:invalid/></xs:schema>")
      except XsdValidationError as e:
          print(e)

   .. note::

      Individual validation failures from :meth:`XsdValidator.validate` and
      :meth:`XsdValidator.validate_str` are returned as :class:`ValidationError`
      objects in a list, **not** raised as exceptions.

Exception hierarchy
-------------------

::

    Exception
    ├── XmlParseError
    ├── XmlWellFormednessError
    ├── XmlNamespaceError
    ├── XPathError
    └── XsdValidationError
