pyuppsala documentation
=======================

**pyuppsala** is a Python binding for the `Uppsala <https://crates.io/crates/uppsala>`_
XML library -- a zero-dependency, pure-Rust implementation of XML 1.0, Namespaces,
XPath 1.0, and XSD validation.

Features
--------

- **XML 1.0 parsing** with namespace support and encoding detection (UTF-8/UTF-16)
- **DOM** with tree traversal, mutation, and serialization
- **Source tracking** -- access original source text and byte ranges for any node
- **XPath 1.0** evaluation (node-sets, strings, numbers, booleans)
- **XSD 1.1 validation** with 44+ built-in types, facets, identity constraints
- **XSD regex** pattern matching
- **XmlWriter** for imperative XML construction
- **SIMD-accelerated** parsing on x86_64
- **Zero Python dependencies** -- ships as a single native extension

.. toctree::
   :maxdepth: 2
   :caption: Contents

   quickstart
   api
   exceptions
   examples
