# pyuppsala

Python bindings for the [Uppsala](https://crates.io/crates/uppsala) XML library
-- a zero-dependency, pure-Rust implementation of XML 1.0, Namespaces, XPath 1.0,
and XSD validation.

pyuppsala gives you a fast, correct, and memory-safe XML toolkit from Python with
no C dependencies to compile and no transitive native libraries to audit.

## Features

- **XML 1.0 parsing** with full well-formedness checking
- **Namespace-aware DOM** with tree mutation (create, append, insert, remove, detach)
- **XPath 1.0** evaluation (all axes, functions, predicates)
- **XSD validation** (structures + datatypes, 40+ built-in types, facets, complex types)
- **XSD regex** pattern matching (Unicode categories, blocks, character class subtraction)
- **Imperative XML builder** (`XmlWriter`) for constructing output without a DOM
- **Serialization** with pretty-printing, compact output, and streaming to files
- **Automatic encoding detection** for UTF-8 and UTF-16 (LE/BE)

Read the [full documentation](https://pyuppsala.rtfd.io)

## Installation

```bash
python3 -m pip install pyuppsala
```

Or with [uv](https://docs.astral.sh/uv/):

```bash
uv add pyuppsala
```

Wheels are compiled from Rust via [maturin](https://www.maturin.rs/).
Python 3.10+ is required.

## Quick start

### Parse and query

```python
from pyuppsala import Document, XPathEvaluator

doc = Document("<bookstore><book><title>Moby Dick</title></book></bookstore>")
doc.prepare_xpath()

xpath = XPathEvaluator()
title = xpath.evaluate(doc, "string(//title)")
print(title)  # "Moby Dick"
```

### Build XML

```python
from pyuppsala import XmlWriter

w = XmlWriter()
w.write_declaration()
w.start_element("catalog", [("xmlns", "urn:example")])
w.start_element("item", [("id", "1")])
w.text("Widget")
w.end_element("item")
w.end_element("catalog")
print(w.to_string())
```

### Validate against an XSD schema

```python
from pyuppsala import XsdValidator

schema = """\
<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="greeting" type="xs:string"/>
</xs:schema>
"""

validator = XsdValidator(schema)
print(validator.is_valid_str("<greeting>Hello!</greeting>"))  # True
print(validator.is_valid_str("<greeting><bad/></greeting>"))  # False
```

### Mutate the DOM

```python
from pyuppsala import Document

doc = Document("<root><a/></root>")
root = doc.document_element
b = doc.create_element("b")
doc.append_child(root, b)
print(doc.to_xml())  # <root><a/><b/></root>
```

### XSD regex

```python
from pyuppsala import XsdRegex

regex = XsdRegex(r"[0-9]{5}")
print(regex.is_match("12345"))  # True
print(regex.is_match("abcde"))  # False
```

## API overview

| Class / function | Purpose |
|---|---|
| `Document(xml)` | Parse XML string into a DOM |
| `Document.from_bytes(data)` | Parse XML bytes (auto-detects UTF-8/UTF-16) |
| `Document.empty()` | Create an empty document for building from scratch |
| `Node` | A handle to a node in the document tree |
| `QName` | A qualified XML name (local name + optional namespace + prefix) |
| `Attribute` | An XML attribute (name + value) |
| `XPathEvaluator` | Evaluate XPath 1.0 expressions |
| `XsdValidator(schema)` | Validate documents against an XSD schema |
| `XmlWriter` | Imperative XML builder (no DOM needed) |
| `XsdRegex(pattern)` | XSD regular expression pattern matcher |
| `parse(xml)` | Module-level shorthand for `Document(xml)` |
| `parse_bytes(data)` | Module-level shorthand for `Document.from_bytes(data)` |

### Exceptions

| Exception | Raised when |
|---|---|
| `XmlParseError` | XML is syntactically malformed |
| `XmlWellFormednessError` | XML violates well-formedness constraints |
| `XmlNamespaceError` | Namespace prefix is undeclared or misused |
| `XPathError` | XPath expression is invalid |
| `XsdValidationError` | XSD schema itself is invalid |

All exceptions inherit from `Exception`.

## Type stubs

A `pyuppsala.pyi` file is included for full IDE auto-completion and
type-checking with mypy/pyright.

## Development

```bash
# Clone the repository
git clone https://github.com/kushaldas/pyuppsala.git
cd pyuppsala

# Set up the environment with uv
uv sync

# Build the native extension in development mode
uv run maturin develop

# Run the test suite
uv run pytest

# Build a release wheel
uv run maturin build --release
```

## License

BSD-2-Clause
