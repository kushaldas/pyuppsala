# Changelog


## 0.7.1

Built against uppsala 0.7.1, a security/hardening release. All of its changes
are enforced transparently by uppsala's defaults, so there is no pyuppsala API
change:

- XSD validation fails closed for unresolved element references, compares
  expanded names for namespace-sensitive attributes and strict wildcards, and
  rejects `xs:unique`/`xs:key`/`xs:keyref` fields selecting more than one node.
- Stricter XSD datatype/facet validation for hostile inputs (instant-based
  date/time facet comparison, no date/time normalization of non-temporal
  enumerations, rejection of malformed negative dates and invalid pattern
  facets, `xs:QName` rejecting unbound prefixes).
- DTD content-model parsing observes the parser nesting-depth limit.
- XSLT-generated comments/PIs reject markup-breaking content, and opt-in EXSLT
  `str:padding()` is capped against attacker-selected output allocation.


## 0.7.0

Built against uppsala 0.7.0, which adds an XSLT 1.0 engine. Highlights:

### Added

- `etree.XSLT` / `_XSLTResultTree` -- XSLT 1.0 transforms via uppsala's engine.
- XInclude processing (`xi:include`, `parse="xml"`/`"text"`, `encoding`,
  `fallback`), with a bounded remote-fetch timeout. Remote `http(s)`/`ftp`
  fetches are opt-in via `xinclude(network_access=True)` (off by default, like
  lxml's `no_network`).
- Native `_ElementBase` fast paths (tag/text/tail/nsmap/len/iter), a native
  descendant iterator, and `import_subtree` for faster cross-document copies.
- `XsdValidator.set_lenient` for libxml2/lxml-compatible built-in datatype
  validation, also exposed as `etree.XMLSchema(..., lenient=True)`.

### Fixed

- `DocumentInvalid.error_log` is now per-instance instead of a shared
  class-level list.


## 0.6.0

Built against uppsala 0.6.0, which brings the SIMD / lookup-table byte-scanning
and `try_reserve` arena pre-allocation parser performance work — pyuppsala
inherits these transparently through the dependency upgrade.

### Build

- The release profile now uses fat LTO and a single codegen unit
  (`[profile.release] lto = "fat"`, `codegen-units = 1`). Since the cdylib is the
  final compiled artifact, this lets uppsala's hot scanning loops inline across
  the crate boundary into pyuppsala's call sites.


## 0.5.1

First released version of the 0.5.x line (0.5.0 was never published). Built
against uppsala 0.5.1.

### Breaking Changes

#### `Node.remove_attribute` now matches on `(local name, namespace)`

`Node.remove_attribute(name, namespace_uri=None)` previously removed the first
attribute with a matching local name regardless of namespace. It now matches the
attribute by both local name and namespace, with `namespace_uri=None` meaning the
attribute that has *no* namespace. A namespaced `{ns}name` is no longer removed by
a bare `remove_attribute("name")` call. This makes the method consistent with
`get_attribute` / `set_attribute`.

#### Stricter XML name and namespace validation

The DOM and writer construction APIs now reject names that are not well-formed
XML, raising `ValueError`:

- `create_element`, `set_attribute`, `set_qname`, and the `QName` constructor
  validate local names and prefixes against the exact XML 1.0
  `NameStartChar` / `NameChar` productions (disallowed code points such as
  U+00D7, U+00F7, and leading combining marks are now rejected; supplementary
  plane code points `U+10000..U+EFFFF` are accepted).
- The XML Namespaces reserved bindings are enforced: a name cannot use the
  `xmlns` prefix, rebind the `xml` prefix or the XML namespace, or sit in the
  `xmlns` namespace, and a prefix without a namespace URI is rejected.
- `XmlWriter.start_element` / `end_element` / `empty_element` /
  `empty_element_expanded` / `processing_instruction` validate element and
  attribute names and PI targets and can now raise `ValueError`.

### Added

#### Opt-in DTD / entity hardening (defusedxml-style)

`parse`, `parse_bytes`, `Document`, and `Document.from_bytes` accept two new
keyword-only arguments (both default off, so existing behavior is unchanged):

- `forbid_dtd=True` rejects any `<!DOCTYPE` declaration at parse time.
- `forbid_entities=True` rejects `<!ENTITY>` declarations (general and
  parameter) while still allowing the rest of a DTD.

The `etree.XMLParser` facade gains the same `forbid_dtd` / `forbid_entities`
options. Backed by uppsala 0.5.1's `Parser::with_forbid_dtd` /
`with_forbid_entities`.

#### `pyuppsala.etree` — an `lxml.etree`-compatible API

A new pure-Python `pyuppsala.etree` submodule layers an `lxml.etree`-compatible
API over the native `Document` / `Node` tree, for near drop-in use by code
written against `lxml.etree`:

```python
from pyuppsala import etree as ET

root = ET.fromstring("<a x='1'><b>hi</b>tail</a>")
ET.SubElement(root, "c").text = "y"
print(ET.tostring(root, encoding="unicode"))
```

- Elements are live views over a backing native `Document`; object identity is
  stable through a per-document proxy cache (`root[0] is root[0]`). `.text` /
  `.tail` map onto Uppsala's sibling text nodes, so serialization is automatic.
- Factories and node types: `Element`, `SubElement`, `Comment`,
  `ProcessingInstruction` / `PI`, `QName`, `ElementTree`.
- I/O: `fromstring` / `XML`, `fromstringlist`, `parse`, `tostring`, `tounicode`,
  `dump`, `indent`, `iselement`.
- Search: `find` / `findall` / `findtext` / `iterfind` delegate to the stdlib
  `xml.etree.ElementPath` (with element-only `*` / `//*` wildcards);
  `.xpath()`, `XPath`, `ETXPath`, and `XPathEvaluator` delegate to the native
  `XPathEvaluator`.
- Parsing and validation: `XMLParser` (mapping lxml options onto Uppsala's
  security knobs), `register_namespace`, `XMLSchema`.
- lxml-named exception hierarchy (`LxmlError`, `XMLSyntaxError` / `ParseError`,
  `XPathError`, `DocumentInvalid`, `XMLSchemaParseError`, ...) wrapping the
  native pyuppsala exceptions.
- Cross-tree `append` / `insert` / `replace` deep-copy the subtree into the
  target document, carry over namespaces inherited from ancestors, and re-point
  live proxies so existing references stay valid.

`XMLParser` rejects unsupported correctness-affecting options
(`recover`, `dtd_validation`, `resolve_entities=False`, custom targets /
resolvers) and unknown keyword arguments with `NotImplementedError` / `TypeError`
rather than silently ignoring them. See `docs/etree.rst` for the supported
feature matrix.

#### Native helpers backing the etree layer

New methods on the native classes, used by `pyuppsala.etree`:
`Node.{node_id, first_child, last_child, next_sibling, previous_sibling,
namespace_declarations, comment_text, pi_target, pi_data, set_text,
set_pi_data, set_qname}` and `Document.set_namespace_declaration`.

#### Typing stubs

Shipped `python/pyuppsala/__init__.pyi` and `etree.pyi` stubs for IDE and
type-checker support.

### Changed

- The native extension is now built as `pyuppsala._pyuppsala` in a mixed
  Rust + Python package layout; `import pyuppsala` and the existing public API
  are unchanged.
- Native exception types now report their `__module__` as the public
  `pyuppsala` package (rather than `_pyuppsala`), so tracebacks and pickling
  resolve correctly.

## 0.4.0

### Added

- Resource-limit knobs surfaced from the Uppsala 0.4.0 security release:
  `parse` / `parse_bytes` / `Document` / `Document.from_bytes` accept
  keyword-only `max_depth`, `max_entity_expansion`, and `namespace_aware`;
  `XPathEvaluator(*, max_depth=...)`; `XsdRegex(pattern, *, max_depth=...)`;
  `XsdRegex.is_match(input, *, max_steps=...)`.
- Module-level constants `DEFAULT_MAX_DEPTH`, `DEFAULT_MAX_ENTITY_EXPANSION`,
  `DEFAULT_MAX_XPATH_DEPTH`, `DEFAULT_MAX_REGEX_GROUP_DEPTH`, and
  `DEFAULT_MAX_REGEX_STEPS`, mirroring Uppsala's defaults.

### Changed

- Updated the Rust backend to `uppsala` 0.4.0, a security release closing 26
  audit findings (11 High severity). Billion-laughs blocking,
  comment / PI / CDATA round-trip injection sanitization, XSD include
  path-traversal mitigation, and polynomial-ReDoS step caps are enforced
  transparently by Uppsala's defaults.

### Fixed

- Hardened byte decoding in `Document.from_bytes`, and kept UTF-16 detection
  working when resource-limit keyword arguments are supplied.

## 0.3.1

### Added

- Exposed the Uppsala 0.3.0 convenience APIs: `Node.element_text`,
  `Node.get_attribute(name, namespace_uri)`,
  `Node.first_child_element_by_name_ns`, `Node.child_elements_by_name_ns`,
  `Node.matches_name_ns`, `Node.source` / `Node.source_range`,
  `Document.input_text`, and `QName.matches(local_name, namespace_uri)`.
- SIMD-accelerated parsing (SSE2 on x86_64) and XSD fixed-value constraint
  support, inherited automatically from the backend with no API change.

## 0.3.0

Initial public release. Python bindings (via PyO3 / maturin) for the Uppsala
pure-Rust XML library, exposing XML parsing, DOM manipulation, XPath 1.0
evaluation, XSD validation, and XSD regex matching with zero Python-side
dependencies. Core classes: `Document`, `Node`, `QName`, `Attribute`,
`XPathEvaluator`, `XsdValidator`, `ValidationError`, `XmlWriter`, and `XsdRegex`.
