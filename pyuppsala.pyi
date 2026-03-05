"""Type stubs for pyuppsala — Python bindings for the Uppsala XML library."""

from __future__ import annotations

from typing import Optional, Union

# Exceptions

class XmlParseError(Exception): ...
class XmlWellFormednessError(Exception): ...
class XmlNamespaceError(Exception): ...
class XPathError(Exception): ...
class XsdValidationError(Exception): ...

# Classes

class QName:
    """A qualified XML name with optional namespace URI and prefix."""

    def __init__(
        self,
        local_name: str,
        namespace_uri: Optional[str] = None,
        prefix: Optional[str] = None,
    ) -> None: ...
    @property
    def local_name(self) -> str: ...
    @property
    def namespace_uri(self) -> Optional[str]: ...
    @property
    def prefix(self) -> Optional[str]: ...
    @property
    def prefixed_name(self) -> str: ...
    def matches(
        self, local_name: str, namespace_uri: Optional[str] = None
    ) -> bool:
        """Check whether this QName matches the given local name and optional namespace URI."""
        ...
    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...
    def __eq__(self, other: QName) -> bool: ...
    def __hash__(self) -> int: ...

class Attribute:
    """An XML attribute with a qualified name and string value."""

    @property
    def name(self) -> QName: ...
    @property
    def value(self) -> str: ...
    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...

class Node:
    """A node within an XML document.

    Nodes are lightweight handles — the actual data lives inside the Document.
    Do not use a Node after its parent Document has been garbage collected.
    """

    @property
    def kind(self) -> str:
        """The kind of this node: "document", "element", "text",
        "comment", "processing_instruction", "cdata", or "attribute"."""
        ...
    @property
    def tag(self) -> Optional[QName]:
        """The tag name for element nodes, or None for other kinds."""
        ...
    @property
    def text(self) -> Optional[str]:
        """The text content for text/comment/cdata nodes, or None."""
        ...
    @property
    def text_content(self) -> str:
        """Recursively collected text content of this node and all descendants."""
        ...
    @property
    def element_text(self) -> Optional[str]:
        """The text of the first Text or CDATA child, or None.

        Fast, zero-copy way to get the text content of simple elements
        like ``<name>value</name>``. Unlike ``text_content``, this does not recurse.
        """
        ...
    @property
    def attributes(self) -> list[Attribute]:
        """The list of attributes for element nodes."""
        ...
    @property
    def parent(self) -> Optional[Node]:
        """The parent node, or None for the root."""
        ...
    @property
    def children(self) -> list[Node]:
        """The child nodes of this node."""
        ...
    @property
    def line(self) -> int:
        """The line number of this node in the source document."""
        ...
    @property
    def column(self) -> int:
        """The column number of this node in the source document."""
        ...
    @property
    def source_range(self) -> Optional[tuple[int, int]]:
        """The byte range (start, end) of this node in the original source, or None.

        Returns None for programmatically created nodes.
        """
        ...
    @property
    def source(self) -> Optional[str]:
        """The original source text of this node, or None.

        Returns None for programmatically created nodes.
        """
        ...
    def get_attribute(
        self, name: str, namespace_uri: Optional[str] = None
    ) -> Optional[str]:
        """Get an attribute value by local name."""
        ...
    def set_attribute(
        self,
        name: str,
        value: str,
        namespace_uri: Optional[str] = None,
        prefix: Optional[str] = None,
    ) -> Optional[str]:
        """Set an attribute value. Returns the previous value if any."""
        ...
    def remove_attribute(self, name: str) -> Optional[str]:
        """Remove an attribute by local name. Returns the old value if any."""
        ...
    def to_xml(self) -> str:
        """Serialize this node and its subtree to XML."""
        ...
    def to_xml_with_options(
        self,
        indent: Optional[str] = None,
        expand_empty_elements: bool = False,
    ) -> str:
        """Serialize this node and its subtree to XML with formatting options."""
        ...
    def get_elements_by_tag_name(self, name: str) -> list[Node]:
        """Find descendant elements by local tag name."""
        ...
    def get_elements_by_tag_name_ns(self, namespace_uri: str, name: str) -> list[Node]:
        """Find descendant elements by namespace URI and local tag name."""
        ...
    def first_child_element_by_name_ns(
        self, namespace_uri: str, local_name: str
    ) -> Optional[Node]:
        """Find the first direct child element matching the given namespace URI and local name."""
        ...
    def child_elements_by_name_ns(
        self, namespace_uri: str, local_name: str
    ) -> list[Node]:
        """Find all direct child elements matching the given namespace URI and local name."""
        ...
    def matches_name_ns(self, namespace_uri: str, local_name: str) -> bool:
        """Check whether this element matches the given namespace URI and local name.

        Returns False for non-element nodes.
        """
        ...
    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> NodeIterator: ...
    def __getitem__(self, index: int) -> Node: ...
    def __bool__(self) -> bool: ...

class NodeIterator:
    """Iterator over child nodes."""

    def __iter__(self) -> NodeIterator: ...
    def __next__(self) -> Node: ...

class Document:
    """An XML document.

    Parse XML with ``Document(xml_string)`` or ``Document.from_bytes(data)``.
    """

    def __init__(self, xml: str) -> None:
        """Parse an XML string into a Document."""
        ...
    @staticmethod
    def from_bytes(data: bytes) -> Document:
        """Parse XML from bytes, with automatic encoding detection (UTF-8/UTF-16)."""
        ...
    @staticmethod
    def empty() -> Document:
        """Create a new empty document."""
        ...
    @property
    def root(self) -> Node:
        """The root node of the document (the Document node itself)."""
        ...
    @property
    def document_element(self) -> Optional[Node]:
        """The document element (the top-level element), or None."""
        ...
    @property
    def input_text(self) -> str:
        """The original input text that was parsed to create this document.

        Returns an empty string for programmatically constructed documents.
        """
        ...
    def get_elements_by_tag_name(self, name: str) -> list[Node]:
        """Find all elements with the given local tag name."""
        ...
    def get_elements_by_tag_name_ns(self, namespace_uri: str, name: str) -> list[Node]:
        """Find all elements with the given namespace URI and local tag name."""
        ...

    # Tree mutation

    def create_element(
        self,
        local_name: str,
        namespace_uri: Optional[str] = None,
        prefix: Optional[str] = None,
    ) -> Node:
        """Create a new element node (not yet attached to the tree)."""
        ...
    def create_text(self, text: str) -> Node:
        """Create a new text node (not yet attached to the tree)."""
        ...
    def create_comment(self, text: str) -> Node:
        """Create a new comment node (not yet attached to the tree)."""
        ...
    def create_cdata(self, text: str) -> Node:
        """Create a new CDATA section node (not yet attached to the tree)."""
        ...
    def create_processing_instruction(
        self, target: str, data: Optional[str] = None
    ) -> Node:
        """Create a new processing instruction node (not yet attached to the tree)."""
        ...
    def append_child(self, parent: Node, child: Node) -> None:
        """Append a child node to a parent node."""
        ...
    def insert_before(self, parent: Node, new_child: Node, reference: Node) -> None:
        """Insert a child node before a reference node."""
        ...
    def insert_after(self, parent: Node, new_child: Node, reference: Node) -> None:
        """Insert a child node after a reference node."""
        ...
    def remove_child(self, parent: Node, child: Node) -> None:
        """Remove a child node from its parent."""
        ...
    def replace_child(self, parent: Node, new_child: Node, old_child: Node) -> None:
        """Replace old_child with new_child under the given parent."""
        ...
    def detach(self, node: Node) -> None:
        """Detach a node from its parent, removing it from the tree.

        The node remains valid and can be re-attached elsewhere.
        """
        ...

    # Serialization

    def to_xml(self) -> str:
        """Serialize the document to a compact XML string."""
        ...
    def to_xml_with_options(
        self,
        indent: Optional[str] = None,
        expand_empty_elements: bool = False,
    ) -> str:
        """Serialize the document to an XML string with formatting options."""
        ...
    def write_to_file(self, path: str) -> None:
        """Write the document to a file."""
        ...

    # XPath

    def prepare_xpath(self) -> None:
        """Prepare the document for XPath evaluation (builds internal indices)."""
        ...
    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...
    def __bool__(self) -> bool: ...

class XPathEvaluator:
    """XPath 1.0 expression evaluator."""

    def __init__(self) -> None: ...
    def add_namespace(self, prefix: str, uri: str) -> None:
        """Register a namespace prefix for use in XPath expressions."""
        ...
    def evaluate(
        self,
        doc: Document,
        expr: str,
        context: Optional[Node] = None,
    ) -> Union[list[Node], bool, float, str]:
        """Evaluate an XPath expression and return the result."""
        ...
    def select(
        self,
        doc: Document,
        expr: str,
        context: Optional[Node] = None,
    ) -> list[Node]:
        """Evaluate an XPath expression and return matching nodes."""
        ...
    def __repr__(self) -> str: ...

class ValidationError:
    """A single XSD validation error with optional location info."""

    message: str
    line: Optional[int]
    column: Optional[int]
    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...

class XsdValidator:
    """XSD 1.1 schema validator."""

    def __init__(self, schema_xml: str) -> None:
        """Create a validator from an XSD schema string."""
        ...
    @staticmethod
    def from_file(schema_xml: str, base_path: str) -> XsdValidator:
        """Create a validator from an XSD schema string with a base path for resolving includes."""
        ...
    def set_enforce_qname_length_facets(self, enforce: bool) -> None:
        """Configure whether QName/NOTATION length facets are enforced."""
        ...
    def validate(self, doc: Document) -> list[ValidationError]:
        """Validate an XML document. Returns a list of errors (empty = valid)."""
        ...
    def validate_str(self, xml: str) -> list[ValidationError]:
        """Validate an XML string. Returns a list of errors (empty = valid)."""
        ...
    def is_valid(self, doc: Document) -> bool:
        """Check if an XML document is valid."""
        ...
    def is_valid_str(self, xml: str) -> bool:
        """Check if an XML string is valid."""
        ...
    def __repr__(self) -> str: ...

class XmlWriter:
    """An imperative XML builder for constructing XML fragments."""

    def __init__(self) -> None: ...
    def write_declaration(self) -> None:
        """Write ``<?xml version="1.0" encoding="UTF-8"?>``."""
        ...
    def write_declaration_full(
        self,
        version: str = "1.0",
        encoding: Optional[str] = None,
        standalone: Optional[bool] = None,
    ) -> None:
        """Write a full XML declaration with custom parameters."""
        ...
    def start_element(
        self, name: str, attrs: Optional[list[tuple[str, str]]] = None
    ) -> None:
        """Start an element with the given name and attributes."""
        ...
    def end_element(self, name: str) -> None:
        """End the current element."""
        ...
    def empty_element(
        self, name: str, attrs: Optional[list[tuple[str, str]]] = None
    ) -> None:
        """Write a self-closing empty element: ``<name/>``."""
        ...
    def empty_element_expanded(
        self, name: str, attrs: Optional[list[tuple[str, str]]] = None
    ) -> None:
        """Write an expanded empty element: ``<name></name>``."""
        ...
    def text(self, content: str) -> None:
        """Write text content (auto-escaped)."""
        ...
    def cdata(self, content: str) -> None:
        """Write a CDATA section."""
        ...
    def comment(self, content: str) -> None:
        """Write a comment."""
        ...
    def processing_instruction(self, target: str, data: Optional[str] = None) -> None:
        """Write a processing instruction."""
        ...
    def raw(self, xml: str) -> None:
        """Write raw XML content (not escaped)."""
        ...
    def to_string(self) -> str:
        """Return the accumulated XML as a string."""
        ...
    def to_bytes(self) -> bytes:
        """Return the accumulated XML as bytes."""
        ...
    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...
    def __len__(self) -> int: ...
    def __bool__(self) -> bool: ...

class XsdRegex:
    """XSD regular expression pattern matcher.

    XSD regexes are implicitly anchored (must match the full string).
    """

    def __init__(self, pattern: str) -> None:
        """Compile an XSD regex pattern."""
        ...
    def is_match(self, input: str) -> bool:
        """Test whether the input string fully matches the pattern."""
        ...
    @property
    def pattern(self) -> str:
        """The original pattern string."""
        ...
    def __repr__(self) -> str: ...
    def __str__(self) -> str: ...

# Module-level functions

def parse(xml: str) -> Document:
    """Parse an XML string and return a Document."""
    ...

def parse_bytes(data: bytes) -> Document:
    """Parse XML bytes and return a Document, with automatic encoding detection."""
    ...
