use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use std::sync::{Arc, Mutex};
use uppsala::dom::{Attribute as UAttribute, NodeId, NodeKind, QName as UQName, XmlWriteOptions};
use uppsala::writer::XmlWriter as UXmlWriter;
use uppsala::xpath::{XPathEvaluator as UXPathEvaluator, XPathValue as UXPathValue};
use uppsala::xsd::XsdValidator as UXsdValidator;
use uppsala::{Document as UDocument, XmlError};

// ---------------------------------------------------------------------------
// Custom Python exceptions
// ---------------------------------------------------------------------------

create_exception!(pyuppsala, XmlParseError, pyo3::exceptions::PyException);
create_exception!(
    pyuppsala,
    XmlWellFormednessError,
    pyo3::exceptions::PyException
);
create_exception!(pyuppsala, XmlNamespaceError, pyo3::exceptions::PyException);
create_exception!(pyuppsala, XPathError, pyo3::exceptions::PyException);
create_exception!(pyuppsala, XsdValidationError, pyo3::exceptions::PyException);

fn xml_error_to_pyerr(e: XmlError) -> PyErr {
    match e {
        XmlError::Parse(ref pe) => {
            XmlParseError::new_err(format!("{}:{}: {}", pe.line, pe.column, pe.message))
        }
        XmlError::WellFormedness(ref we) => {
            XmlWellFormednessError::new_err(format!("{}:{}: {}", we.line, we.column, we.message))
        }
        XmlError::Namespace(ref ne) => {
            XmlNamespaceError::new_err(format!("{}:{}: {}", ne.line, ne.column, ne.message))
        }
        XmlError::XPath(ref xe) => XPathError::new_err(xe.message.clone()),
        XmlError::Validation(ref ve) => {
            let loc = match (ve.line, ve.column) {
                (Some(l), Some(c)) => format!("{}:{}: ", l, c),
                (Some(l), None) => format!("{}: ", l),
                _ => String::new(),
            };
            XsdValidationError::new_err(format!("{}{}", loc, ve.message))
        }
        XmlError::UnexpectedEof => XmlParseError::new_err("Unexpected end of input".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Shared document handle — allows multiple Python objects to reference one DOM
// ---------------------------------------------------------------------------

/// Wraps a Document alongside the original input text.
///
/// `into_static()` drops the original input reference from the Document,
/// so we store it separately to support `input_text()`, `node_source()`,
/// and `node_range()`.
struct DocWithInput {
    doc: UDocument<'static>,
    input: String,
}

type SharedDoc = Arc<Mutex<DocWithInput>>;

// ---------------------------------------------------------------------------
// QName — Python wrapper
// ---------------------------------------------------------------------------

/// A qualified XML name with optional namespace URI and prefix.
#[pyclass(name = "QName", from_py_object)]
#[derive(Clone)]
struct QName {
    namespace_uri: Option<String>,
    prefix: Option<String>,
    local_name: String,
}

#[pymethods]
impl QName {
    #[new]
    #[pyo3(signature = (local_name, namespace_uri=None, prefix=None))]
    fn new(local_name: String, namespace_uri: Option<String>, prefix: Option<String>) -> Self {
        QName {
            namespace_uri,
            prefix,
            local_name,
        }
    }

    /// The local part of the name.
    #[getter]
    fn local_name(&self) -> &str {
        &self.local_name
    }

    /// The namespace URI, or None.
    #[getter]
    fn namespace_uri(&self) -> Option<&str> {
        self.namespace_uri.as_deref()
    }

    /// The namespace prefix, or None.
    #[getter]
    fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// The prefixed name (e.g. "soap:Envelope" or just "root").
    #[getter]
    fn prefixed_name(&self) -> String {
        match &self.prefix {
            Some(p) => format!("{}:{}", p, self.local_name),
            None => self.local_name.clone(),
        }
    }

    /// Check whether this QName matches the given local name and optional namespace URI.
    #[pyo3(signature = (local_name, namespace_uri=None))]
    fn matches(&self, local_name: &str, namespace_uri: Option<&str>) -> bool {
        self.local_name == local_name && self.namespace_uri.as_deref() == namespace_uri
    }

    fn __repr__(&self) -> String {
        match (&self.namespace_uri, &self.prefix) {
            (Some(ns), Some(p)) => {
                format!(
                    "QName('{}', namespace_uri='{}', prefix='{}')",
                    self.local_name, ns, p
                )
            }
            (Some(ns), None) => {
                format!("QName('{}', namespace_uri='{}')", self.local_name, ns)
            }
            _ => format!("QName('{}')", self.local_name),
        }
    }

    fn __str__(&self) -> String {
        self.prefixed_name()
    }

    fn __eq__(&self, other: &QName) -> bool {
        self.local_name == other.local_name && self.namespace_uri == other.namespace_uri
    }

    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.local_name.hash(&mut hasher);
        self.namespace_uri.hash(&mut hasher);
        hasher.finish()
    }
}

impl QName {
    fn from_uqname(q: &UQName<'_>) -> Self {
        QName {
            namespace_uri: q.namespace_uri.as_ref().map(|s| s.to_string()),
            prefix: q.prefix.as_ref().map(|s| s.to_string()),
            local_name: q.local_name.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Attribute — Python wrapper
// ---------------------------------------------------------------------------

/// An XML attribute with a qualified name and string value.
#[pyclass(name = "Attribute", from_py_object)]
#[derive(Clone)]
struct Attribute {
    name: QName,
    value: String,
}

#[pymethods]
impl Attribute {
    /// The qualified name of this attribute.
    #[getter]
    fn name(&self) -> QName {
        self.name.clone()
    }

    /// The attribute value.
    #[getter]
    fn value(&self) -> &str {
        &self.value
    }

    fn __repr__(&self) -> String {
        format!("Attribute({}='{}')", self.name.__str__(), self.value)
    }

    fn __str__(&self) -> String {
        format!("{}=\"{}\"", self.name.__str__(), self.value)
    }
}

impl Attribute {
    fn from_uattr(a: &UAttribute<'_>) -> Self {
        Attribute {
            name: QName::from_uqname(&a.name),
            value: a.value.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Node — a lightweight handle into a Document
// ---------------------------------------------------------------------------

/// A node within an XML document.
///
/// Nodes are lightweight handles — the actual data lives inside the Document.
/// Do not use a Node after its parent Document has been garbage collected.
#[pyclass(name = "Node", from_py_object)]
#[derive(Clone)]
struct Node {
    doc: SharedDoc,
    id: NodeId,
}

#[pymethods]
impl Node {
    /// The kind of this node as a string: "document", "element", "text",
    /// "comment", "processing_instruction", "cdata", or "attribute".
    #[getter]
    fn kind(&self) -> PyResult<String> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind(self.id) {
            Some(NodeKind::Document) => Ok("document".into()),
            Some(NodeKind::Element(_)) => Ok("element".into()),
            Some(NodeKind::Text(_)) => Ok("text".into()),
            Some(NodeKind::Comment(_)) => Ok("comment".into()),
            Some(NodeKind::ProcessingInstruction(_)) => Ok("processing_instruction".into()),
            Some(NodeKind::CData(_)) => Ok("cdata".into()),
            Some(NodeKind::Attribute(_, _)) => Ok("attribute".into()),
            None => Err(PyValueError::new_err("Invalid node")),
        }
    }

    /// The tag name (QName) for element nodes, or None for other node kinds.
    #[getter]
    fn tag(&self) -> PyResult<Option<QName>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .element(self.id)
            .map(|el| QName::from_uqname(&el.name)))
    }

    /// The text content for text/comment/cdata nodes, or None.
    #[getter]
    fn text(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.text_content(self.id).map(|s| s.to_string()))
    }

    /// Recursively collected text content of this node and all descendants.
    #[getter]
    fn text_content(&self) -> PyResult<String> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.text_content_deep(self.id))
    }

    /// The text of the first Text or CDATA child, or None.
    ///
    /// This is a fast, zero-copy way to get the text content of simple elements
    /// like `<name>value</name>`. Unlike `text_content`, this does not recurse.
    #[getter]
    fn element_text(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.element_text(self.id).map(|s| s.to_string()))
    }

    /// The list of attributes for element nodes.
    #[getter]
    fn attributes(&self) -> PyResult<Vec<Attribute>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element(self.id) {
            Some(el) => Ok(el.attributes.iter().map(Attribute::from_uattr).collect()),
            None => Ok(Vec::new()),
        }
    }

    /// Get an attribute value by local name.
    #[pyo3(signature = (name, namespace_uri=None))]
    fn get_attribute(&self, name: &str, namespace_uri: Option<&str>) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match namespace_uri {
            Some(ns) => Ok(guard
                .doc
                .get_attribute_ns(self.id, ns, name)
                .map(|s| s.to_string())),
            None => Ok(guard
                .doc
                .get_attribute(self.id, name)
                .map(|s| s.to_string())),
        }
    }

    /// Set an attribute value. Returns the previous value if any.
    #[pyo3(signature = (name, value, namespace_uri=None, prefix=None))]
    fn set_attribute(
        &self,
        name: &str,
        value: &str,
        namespace_uri: Option<&str>,
        prefix: Option<&str>,
    ) -> PyResult<Option<String>> {
        let mut guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element_mut(self.id) {
            Some(el) => {
                let qname = match (namespace_uri, prefix) {
                    (Some(ns), Some(p)) => {
                        UQName::full(p.to_string(), ns.to_string(), name.to_string())
                    }
                    (Some(ns), None) => UQName::with_namespace(ns.to_string(), name.to_string()),
                    _ => UQName::local(name.to_string()),
                };
                let old = el.set_attribute(qname, std::borrow::Cow::Owned(value.to_string()));
                Ok(old.map(|s| s.to_string()))
            }
            None => Err(PyValueError::new_err("Node is not an element")),
        }
    }

    /// Remove an attribute by local name. Returns the old value if any.
    fn remove_attribute(&self, name: &str) -> PyResult<Option<String>> {
        let mut guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element_mut(self.id) {
            Some(el) => Ok(el.remove_attribute(name).map(|s| s.to_string())),
            None => Err(PyValueError::new_err("Node is not an element")),
        }
    }

    /// The parent node, or None for the root.
    #[getter]
    fn parent(&self) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.parent(self.id).map(|pid| Node {
            doc: Arc::clone(&self.doc),
            id: pid,
        }))
    }

    /// The child nodes of this node.
    #[getter]
    fn children(&self) -> PyResult<Vec<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .children(self.id)
            .into_iter()
            .map(|cid| Node {
                doc: Arc::clone(&self.doc),
                id: cid,
            })
            .collect())
    }

    /// The line number of this node in the source document (1-based).
    #[getter]
    fn line(&self) -> PyResult<usize> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let byte_pos = match guard.doc.node_range(self.id) {
            Some(r) => r.start,
            None => return Ok(1),
        };
        if guard.input.is_empty() || byte_pos == 0 {
            return Ok(1);
        }
        Ok(guard.input.as_bytes()[..byte_pos]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1)
    }

    /// The column number of this node in the source document (1-based).
    #[getter]
    fn column(&self) -> PyResult<usize> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let byte_pos = match guard.doc.node_range(self.id) {
            Some(r) => r.start,
            None => return Ok(1),
        };
        if guard.input.is_empty() || byte_pos == 0 {
            return Ok(1);
        }
        let bytes = &guard.input.as_bytes()[..byte_pos];
        Ok(match bytes.iter().rposition(|&b| b == b'\n') {
            Some(nl_pos) => byte_pos - nl_pos,
            None => byte_pos + 1,
        })
    }

    /// The byte range (start, end) of this node in the original source, or None.
    ///
    /// Returns None for programmatically created nodes.
    #[getter]
    fn source_range(&self) -> PyResult<Option<(usize, usize)>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .node_range(self.id)
            .map(|r| (r.start, r.end)))
    }

    /// The original source text of this node, or None.
    ///
    /// Returns None for programmatically created nodes.
    #[getter]
    fn source(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_range(self.id) {
            Some(range) if range.end <= guard.input.len() => {
                Ok(Some(guard.input[range].to_string()))
            }
            _ => Ok(None),
        }
    }

    /// Serialize this node and its subtree to XML.
    fn to_xml(&self) -> PyResult<String> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.node_to_xml(self.id))
    }

    /// Serialize this node and its subtree to XML with formatting options.
    #[pyo3(signature = (indent=None, expand_empty_elements=false))]
    fn to_xml_with_options(
        &self,
        indent: Option<&str>,
        expand_empty_elements: bool,
    ) -> PyResult<String> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let opts = make_write_options(indent, expand_empty_elements);
        Ok(guard.doc.node_to_xml_with_options(self.id, &opts))
    }

    /// Find descendant elements by local tag name.
    fn get_elements_by_tag_name(&self, name: &str) -> PyResult<Vec<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .get_elements_by_tag_name(name)
            .into_iter()
            .map(|nid| Node {
                doc: Arc::clone(&self.doc),
                id: nid,
            })
            .collect())
    }

    /// Find descendant elements by namespace URI and local tag name.
    fn get_elements_by_tag_name_ns(&self, namespace_uri: &str, name: &str) -> PyResult<Vec<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .get_elements_by_tag_name_ns(namespace_uri, name)
            .into_iter()
            .map(|nid| Node {
                doc: Arc::clone(&self.doc),
                id: nid,
            })
            .collect())
    }

    /// Find the first direct child element matching the given namespace URI and local name.
    fn first_child_element_by_name_ns(
        &self,
        namespace_uri: &str,
        local_name: &str,
    ) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .first_child_element_by_name_ns(self.id, namespace_uri, local_name)
            .map(|nid| Node {
                doc: Arc::clone(&self.doc),
                id: nid,
            }))
    }

    /// Find all direct child elements matching the given namespace URI and local name.
    fn child_elements_by_name_ns(
        &self,
        namespace_uri: &str,
        local_name: &str,
    ) -> PyResult<Vec<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .child_elements_by_name_ns(self.id, namespace_uri, local_name)
            .into_iter()
            .map(|nid| Node {
                doc: Arc::clone(&self.doc),
                id: nid,
            })
            .collect())
    }

    /// Check whether this element matches the given namespace URI and local name.
    ///
    /// Returns False for non-element nodes.
    fn matches_name_ns(&self, namespace_uri: &str, local_name: &str) -> PyResult<bool> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element(self.id) {
            Some(el) => Ok(el.matches_name_ns(namespace_uri, local_name)),
            None => Ok(false),
        }
    }

    fn __repr__(&self) -> PyResult<String> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind(self.id) {
            Some(NodeKind::Element(el)) => Ok(format!("Node(<{}>)", el.name.prefixed_name())),
            Some(NodeKind::Text(t)) => {
                let preview: String = t.chars().take(30).collect();
                Ok(format!("Node(text='{}')", preview))
            }
            Some(NodeKind::Comment(c)) => {
                let preview: String = c.chars().take(30).collect();
                Ok(format!("Node(comment='{}')", preview))
            }
            Some(NodeKind::Document) => Ok("Node(document)".into()),
            Some(NodeKind::CData(cd)) => {
                let preview: String = cd.chars().take(30).collect();
                Ok(format!("Node(cdata='{}')", preview))
            }
            Some(NodeKind::ProcessingInstruction(pi)) => Ok(format!("Node(pi='{}')", pi.target)),
            Some(NodeKind::Attribute(q, _)) => Ok(format!("Node(attr='{}')", q.prefixed_name())),
            None => Ok("Node(invalid)".into()),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        self.to_xml()
    }

    /// Number of child nodes.
    fn __len__(&self) -> PyResult<usize> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.children(self.id).len())
    }

    /// Iterate over child nodes.
    fn __iter__(&self) -> PyResult<NodeIterator> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let children: Vec<NodeId> = guard.doc.children(self.id);
        Ok(NodeIterator {
            doc: Arc::clone(&self.doc),
            ids: children,
            index: 0,
        })
    }

    /// Get a child node by index.
    fn __getitem__(&self, index: isize) -> PyResult<Node> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let children = guard.doc.children(self.id);
        let len = children.len() as isize;
        let idx = if index < 0 { len + index } else { index };
        if idx < 0 || idx >= len {
            return Err(pyo3::exceptions::PyIndexError::new_err(
                "child index out of range",
            ));
        }
        Ok(Node {
            doc: Arc::clone(&self.doc),
            id: children[idx as usize],
        })
    }

    fn __bool__(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// NodeIterator
// ---------------------------------------------------------------------------

#[pyclass]
struct NodeIterator {
    doc: SharedDoc,
    ids: Vec<NodeId>,
    index: usize,
}

#[pymethods]
impl NodeIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> Option<Node> {
        if self.index < self.ids.len() {
            let id = self.ids[self.index];
            self.index += 1;
            Some(Node {
                doc: Arc::clone(&self.doc),
                id,
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Document — Python wrapper
// ---------------------------------------------------------------------------

/// An XML document.
///
/// Parse XML with `Document(xml_string)` or `Document.from_bytes(data)`.
/// The document owns all nodes; use `root`, `document_element`, and tree
/// traversal methods to navigate the DOM.
#[pyclass(name = "Document")]
struct Document {
    inner: SharedDoc,
}

#[pymethods]
impl Document {
    /// Parse an XML string into a Document.
    #[new]
    fn new(xml: &str) -> PyResult<Self> {
        let input = xml.to_string();
        let doc = uppsala::parse(xml)
            .map_err(xml_error_to_pyerr)?
            .into_static();
        Ok(Document {
            inner: Arc::new(Mutex::new(DocWithInput { doc, input })),
        })
    }

    /// Parse XML from bytes, with automatic encoding detection (UTF-8/UTF-16).
    #[staticmethod]
    fn from_bytes(data: &[u8]) -> PyResult<Document> {
        // Decode to get the input text for source tracking
        let input = String::from_utf8_lossy(data).into_owned();
        let doc = uppsala::parse_bytes(data).map_err(xml_error_to_pyerr)?;
        Ok(Document {
            inner: Arc::new(Mutex::new(DocWithInput { doc, input })),
        })
    }

    /// Create a new empty document.
    #[staticmethod]
    fn empty() -> PyResult<Document> {
        let doc = UDocument::new().into_static();
        Ok(Document {
            inner: Arc::new(Mutex::new(DocWithInput {
                doc,
                input: String::new(),
            })),
        })
    }

    /// The root node of the document (the Document node itself).
    #[getter]
    fn root(&self) -> PyResult<Node> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: guard.doc.root(),
        })
    }

    /// The document element (the top-level element), or None.
    #[getter]
    fn document_element(&self) -> PyResult<Option<Node>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.document_element().map(|id| Node {
            doc: Arc::clone(&self.inner),
            id,
        }))
    }

    /// The original input text that was parsed to create this document.
    ///
    /// Returns an empty string for programmatically constructed documents.
    #[getter]
    fn input_text(&self) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.input.clone())
    }

    /// Find all elements with the given local tag name.
    fn get_elements_by_tag_name(&self, name: &str) -> PyResult<Vec<Node>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .get_elements_by_tag_name(name)
            .into_iter()
            .map(|nid| Node {
                doc: Arc::clone(&self.inner),
                id: nid,
            })
            .collect())
    }

    /// Find all elements with the given namespace URI and local tag name.
    fn get_elements_by_tag_name_ns(&self, namespace_uri: &str, name: &str) -> PyResult<Vec<Node>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard
            .doc
            .get_elements_by_tag_name_ns(namespace_uri, name)
            .into_iter()
            .map(|nid| Node {
                doc: Arc::clone(&self.inner),
                id: nid,
            })
            .collect())
    }

    // -- Tree mutation -------------------------------------------------------

    /// Create a new element node (not yet attached to the tree).
    #[pyo3(signature = (local_name, namespace_uri=None, prefix=None))]
    fn create_element(
        &self,
        local_name: &str,
        namespace_uri: Option<&str>,
        prefix: Option<&str>,
    ) -> PyResult<Node> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let qname = match (namespace_uri, prefix) {
            (Some(ns), Some(p)) => {
                UQName::full(p.to_string(), ns.to_string(), local_name.to_string())
            }
            (Some(ns), None) => UQName::with_namespace(ns.to_string(), local_name.to_string()),
            _ => UQName::local(local_name.to_string()),
        };
        let nid = guard.doc.create_element(qname);
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: nid,
        })
    }

    /// Create a new text node (not yet attached to the tree).
    fn create_text(&self, text: &str) -> PyResult<Node> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let nid = guard.doc.create_text(text.to_string());
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: nid,
        })
    }

    /// Create a new comment node (not yet attached to the tree).
    fn create_comment(&self, text: &str) -> PyResult<Node> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let nid = guard.doc.create_comment(text.to_string());
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: nid,
        })
    }

    /// Create a new CDATA section node (not yet attached to the tree).
    fn create_cdata(&self, text: &str) -> PyResult<Node> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let nid = guard.doc.create_cdata(text.to_string());
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: nid,
        })
    }

    /// Create a new processing instruction node (not yet attached to the tree).
    fn create_processing_instruction(&self, target: &str, data: Option<&str>) -> PyResult<Node> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let nid = guard.doc.create_processing_instruction(
            target.to_string(),
            data.map(|s| std::borrow::Cow::Owned(s.to_string())),
        );
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: nid,
        })
    }

    /// Append a child node to a parent node.
    fn append_child(&self, parent: &Node, child: &Node) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.append_child(parent.id, child.id);
        Ok(())
    }

    /// Insert a child node before a reference node.
    fn insert_before(&self, parent: &Node, new_child: &Node, reference: &Node) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard
            .doc
            .insert_before(parent.id, new_child.id, reference.id);
        Ok(())
    }

    /// Insert a child node after a reference node.
    fn insert_after(&self, parent: &Node, new_child: &Node, reference: &Node) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard
            .doc
            .insert_after(parent.id, new_child.id, reference.id);
        Ok(())
    }

    /// Remove a child node from its parent.
    fn remove_child(&self, parent: &Node, child: &Node) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.remove_child(parent.id, child.id);
        Ok(())
    }

    /// Replace old_child with new_child under the given parent.
    fn replace_child(&self, parent: &Node, new_child: &Node, old_child: &Node) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard
            .doc
            .replace_child(parent.id, new_child.id, old_child.id);
        Ok(())
    }

    /// Detach a node from its parent, removing it from the tree.
    ///
    /// The node remains valid and can be re-attached elsewhere with
    /// append_child, insert_before, or insert_after.
    fn detach(&self, node: &Node) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.detach(node.id);
        Ok(())
    }

    // -- Serialization -------------------------------------------------------

    /// Serialize the document to a compact XML string.
    fn to_xml(&self) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.to_xml())
    }

    /// Serialize the document to an XML string with formatting options.
    ///
    /// Args:
    ///     indent: Indentation string (e.g. "  " for 2-space indent), or None for compact.
    ///     expand_empty_elements: If True, write <foo></foo> instead of <foo/>.
    #[pyo3(signature = (indent=None, expand_empty_elements=false))]
    fn to_xml_with_options(
        &self,
        indent: Option<&str>,
        expand_empty_elements: bool,
    ) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let opts = make_write_options(indent, expand_empty_elements);
        Ok(guard.doc.to_xml_with_options(&opts))
    }

    /// Write the document to a file.
    fn write_to_file(&self, path: &str) -> PyResult<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut file = std::fs::File::create(path)
            .map_err(|e| PyRuntimeError::new_err(format!("Cannot create file: {}", e)))?;
        guard
            .doc
            .write_to(&mut file)
            .map_err(|e| PyRuntimeError::new_err(format!("Write error: {}", e)))?;
        Ok(())
    }

    // -- XPath ---------------------------------------------------------------

    /// Prepare the document for XPath evaluation (builds internal indices).
    fn prepare_xpath(&self) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.prepare_xpath();
        Ok(())
    }

    // -- Dunder methods -------------------------------------------------------

    fn __str__(&self) -> PyResult<String> {
        self.to_xml()
    }

    fn __repr__(&self) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let root_el = guard.doc.document_element();
        match root_el {
            Some(id) => {
                if let Some(el) = guard.doc.element(id) {
                    Ok(format!("Document(<{}>)", el.name.prefixed_name()))
                } else {
                    Ok("Document(empty)".into())
                }
            }
            None => Ok("Document(empty)".into()),
        }
    }

    fn __bool__(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// XPathEvaluator
// ---------------------------------------------------------------------------

/// XPath 1.0 expression evaluator.
///
/// Create an evaluator, optionally register namespace prefixes, then
/// call `evaluate()` or `select()` to query a document.
#[pyclass(name = "XPathEvaluator")]
struct XPathEvaluator {
    inner: UXPathEvaluator,
}

#[pymethods]
impl XPathEvaluator {
    #[new]
    fn new() -> Self {
        XPathEvaluator {
            inner: UXPathEvaluator::new(),
        }
    }

    /// Register a namespace prefix for use in XPath expressions.
    fn add_namespace(&mut self, prefix: &str, uri: &str) {
        self.inner.add_namespace(prefix, uri);
    }

    /// Evaluate an XPath expression and return the result.
    ///
    /// Returns a Python object: list of Nodes, bool, float, or str
    /// depending on the XPath result type.
    #[pyo3(signature = (doc, expr, context=None))]
    fn evaluate<'py>(
        &self,
        py: Python<'py>,
        doc: &Document,
        expr: &str,
        context: Option<&Node>,
    ) -> PyResult<Py<PyAny>> {
        let inner_doc = doc
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let ctx_id = match context {
            Some(n) => n.id,
            None => inner_doc.doc.root(),
        };
        let result = self
            .inner
            .evaluate(&inner_doc.doc, ctx_id, expr)
            .map_err(xml_error_to_pyerr)?;
        drop(inner_doc); // release lock before building Python objects
        xpath_value_to_py(py, &doc.inner, result)
    }

    /// Evaluate an XPath expression and return matching nodes.
    #[pyo3(signature = (doc, expr, context=None))]
    fn select(&self, doc: &Document, expr: &str, context: Option<&Node>) -> PyResult<Vec<Node>> {
        let inner_doc = doc
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let ctx_id = match context {
            Some(n) => n.id,
            None => inner_doc.doc.root(),
        };
        let nodes = self
            .inner
            .select_nodes(&inner_doc.doc, ctx_id, expr)
            .map_err(xml_error_to_pyerr)?;
        Ok(nodes
            .into_iter()
            .map(|nid| Node {
                doc: Arc::clone(&doc.inner),
                id: nid,
            })
            .collect())
    }

    fn __repr__(&self) -> String {
        "XPathEvaluator()".into()
    }
}

fn xpath_value_to_py(py: Python<'_>, doc: &SharedDoc, value: UXPathValue) -> PyResult<Py<PyAny>> {
    match value {
        UXPathValue::Boolean(b) => Ok(b.into_pyobject(py)?.to_owned().into_any().unbind()),
        UXPathValue::Number(n) => Ok(n.into_pyobject(py)?.into_any().unbind()),
        UXPathValue::String(s) => Ok(s.into_pyobject(py)?.into_any().unbind()),
        UXPathValue::NodeSet(ids) => {
            let nodes: Vec<Node> = ids
                .into_iter()
                .map(|nid| Node {
                    doc: Arc::clone(doc),
                    id: nid,
                })
                .collect();
            Ok(nodes.into_pyobject(py)?.into_any().unbind())
        }
    }
}

// ---------------------------------------------------------------------------
// ValidationError (Python class for individual XSD errors)
// ---------------------------------------------------------------------------

/// A single XSD validation error with optional location info.
#[pyclass(name = "ValidationError", from_py_object)]
#[derive(Clone)]
struct ValidationErrorPy {
    #[pyo3(get)]
    message: String,
    #[pyo3(get)]
    line: Option<usize>,
    #[pyo3(get)]
    column: Option<usize>,
}

#[pymethods]
impl ValidationErrorPy {
    fn __repr__(&self) -> String {
        match (self.line, self.column) {
            (Some(l), Some(c)) => format!(
                "ValidationError('{}', line={}, column={})",
                self.message, l, c
            ),
            (Some(l), None) => format!("ValidationError('{}', line={})", self.message, l),
            _ => format!("ValidationError('{}')", self.message),
        }
    }

    fn __str__(&self) -> String {
        match (self.line, self.column) {
            (Some(l), Some(c)) => format!("{}:{}: {}", l, c, self.message),
            (Some(l), None) => format!("{}: {}", l, self.message),
            _ => self.message.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// XsdValidator
// ---------------------------------------------------------------------------

/// XSD 1.1 schema validator.
///
/// Load a schema from an XML string, then validate instance documents.
#[pyclass(name = "XsdValidator")]
struct XsdValidator {
    inner: UXsdValidator,
}

#[pymethods]
impl XsdValidator {
    /// Create a validator from an XSD schema string.
    #[new]
    fn new(schema_xml: &str) -> PyResult<Self> {
        let schema_doc = uppsala::parse(schema_xml)
            .map_err(xml_error_to_pyerr)?
            .into_static();
        let validator = UXsdValidator::from_schema(&schema_doc).map_err(xml_error_to_pyerr)?;
        Ok(XsdValidator { inner: validator })
    }

    /// Create a validator from an XSD schema string, resolving external
    /// includes/imports relative to the given base path.
    #[staticmethod]
    fn from_file(schema_xml: &str, base_path: &str) -> PyResult<XsdValidator> {
        let schema_doc = uppsala::parse(schema_xml)
            .map_err(xml_error_to_pyerr)?
            .into_static();
        let path = std::path::Path::new(base_path);
        let validator = UXsdValidator::from_schema_with_base_path(&schema_doc, Some(path))
            .map_err(xml_error_to_pyerr)?;
        Ok(XsdValidator { inner: validator })
    }

    /// Configure whether QName/NOTATION length facets are enforced.
    fn set_enforce_qname_length_facets(&mut self, enforce: bool) {
        self.inner.set_enforce_qname_length_facets(enforce);
    }

    /// Validate an XML document against this schema.
    ///
    /// Returns a list of ValidationError objects. An empty list means valid.
    fn validate(&self, doc: &Document) -> PyResult<Vec<ValidationErrorPy>> {
        let inner_doc = doc
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let errors = self.inner.validate(&inner_doc.doc);
        Ok(errors
            .into_iter()
            .map(|e| ValidationErrorPy {
                message: e.message,
                line: e.line,
                column: e.column,
            })
            .collect())
    }

    /// Validate an XML string against this schema. Convenience method.
    ///
    /// Returns a list of ValidationError objects. An empty list means valid.
    fn validate_str(&self, xml: &str) -> PyResult<Vec<ValidationErrorPy>> {
        let doc = uppsala::parse(xml)
            .map_err(xml_error_to_pyerr)?
            .into_static();
        let errors = self.inner.validate(&doc);
        Ok(errors
            .into_iter()
            .map(|e| ValidationErrorPy {
                message: e.message,
                line: e.line,
                column: e.column,
            })
            .collect())
    }

    /// Check if an XML document is valid. Returns True/False.
    fn is_valid(&self, doc: &Document) -> PyResult<bool> {
        let inner_doc = doc
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(self.inner.validate(&inner_doc.doc).is_empty())
    }

    /// Check if an XML string is valid. Returns True/False.
    fn is_valid_str(&self, xml: &str) -> PyResult<bool> {
        let doc = match uppsala::parse(xml) {
            Ok(d) => d.into_static(),
            Err(_) => return Ok(false),
        };
        Ok(self.inner.validate(&doc).is_empty())
    }

    fn __repr__(&self) -> String {
        "XsdValidator(...)".into()
    }
}

// ---------------------------------------------------------------------------
// XmlWriter — imperative XML builder
// ---------------------------------------------------------------------------

/// An imperative XML builder for constructing XML fragments.
///
/// Use this when you want to build XML output without creating a full DOM.
#[pyclass(name = "XmlWriter")]
struct XmlWriter {
    inner: UXmlWriter,
}

#[pymethods]
impl XmlWriter {
    #[new]
    fn new() -> Self {
        XmlWriter {
            inner: UXmlWriter::new(),
        }
    }

    /// Write an XML declaration: <?xml version="1.0" encoding="UTF-8"?>
    fn write_declaration(&mut self) {
        self.inner.write_declaration();
    }

    /// Write a full XML declaration with custom version, encoding, and standalone.
    #[pyo3(signature = (version="1.0", encoding=None, standalone=None))]
    fn write_declaration_full(
        &mut self,
        version: &str,
        encoding: Option<&str>,
        standalone: Option<bool>,
    ) {
        self.inner
            .write_declaration_full(version, encoding, standalone);
    }

    /// Start an element with the given name and attributes.
    ///
    /// Attributes should be a list of (name, value) tuples.
    #[pyo3(signature = (name, attrs=None))]
    fn start_element(&mut self, name: &str, attrs: Option<Vec<(String, String)>>) {
        let attr_refs: Vec<(&str, &str)> = match &attrs {
            Some(a) => a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
            None => Vec::new(),
        };
        self.inner.start_element(name, &attr_refs);
    }

    /// End the current element.
    fn end_element(&mut self, name: &str) {
        self.inner.end_element(name);
    }

    /// Write a self-closing empty element: <name/>
    #[pyo3(signature = (name, attrs=None))]
    fn empty_element(&mut self, name: &str, attrs: Option<Vec<(String, String)>>) {
        let attr_refs: Vec<(&str, &str)> = match &attrs {
            Some(a) => a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
            None => Vec::new(),
        };
        self.inner.empty_element(name, &attr_refs);
    }

    /// Write an expanded empty element: <name></name>
    #[pyo3(signature = (name, attrs=None))]
    fn empty_element_expanded(&mut self, name: &str, attrs: Option<Vec<(String, String)>>) {
        let attr_refs: Vec<(&str, &str)> = match &attrs {
            Some(a) => a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
            None => Vec::new(),
        };
        self.inner.empty_element_expanded(name, &attr_refs);
    }

    /// Write text content (auto-escaped).
    fn text(&mut self, content: &str) {
        self.inner.text(content);
    }

    /// Write a CDATA section.
    fn cdata(&mut self, content: &str) {
        self.inner.cdata(content);
    }

    /// Write a comment.
    fn comment(&mut self, content: &str) {
        self.inner.comment(content);
    }

    /// Write a processing instruction.
    fn processing_instruction(&mut self, target: &str, data: Option<&str>) {
        self.inner.processing_instruction(target, data);
    }

    /// Write raw XML content (not escaped).
    fn raw(&mut self, xml: &str) {
        self.inner.raw(xml);
    }

    /// Return the accumulated XML as a string.
    fn to_string(&self) -> String {
        self.inner.as_str().to_string()
    }

    /// Return the accumulated XML as bytes.
    fn to_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, pyo3::types::PyBytes> {
        pyo3::types::PyBytes::new(py, self.inner.as_str().as_bytes())
    }

    fn __str__(&self) -> String {
        self.inner.as_str().to_string()
    }

    fn __repr__(&self) -> String {
        format!("XmlWriter(len={})", self.inner.len())
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __bool__(&self) -> bool {
        !self.inner.is_empty()
    }
}

// ---------------------------------------------------------------------------
// XsdRegex
// ---------------------------------------------------------------------------

/// XSD regular expression pattern matcher.
///
/// Compiles an XSD regex pattern and tests strings against it.
/// XSD regexes are implicitly anchored (must match the full string).
#[pyclass(name = "XsdRegex")]
struct XsdRegex {
    inner: uppsala::xsd_regex::XsdRegex,
    pattern: String,
}

#[pymethods]
impl XsdRegex {
    /// Compile an XSD regex pattern.
    #[new]
    fn new(pattern: &str) -> PyResult<Self> {
        let inner = uppsala::xsd_regex::XsdRegex::compile(pattern)
            .map_err(|e| PyValueError::new_err(format!("Invalid XSD regex: {}", e)))?;
        Ok(XsdRegex {
            inner,
            pattern: pattern.to_string(),
        })
    }

    /// Test whether the input string fully matches the pattern.
    fn is_match(&self, input: &str) -> bool {
        self.inner.is_match(input)
    }

    /// The original pattern string.
    #[getter]
    fn pattern(&self) -> &str {
        &self.pattern
    }

    fn __repr__(&self) -> String {
        format!("XsdRegex('{}')", self.pattern)
    }

    fn __str__(&self) -> &str {
        &self.pattern
    }
}

// ---------------------------------------------------------------------------
// Module-level convenience functions
// ---------------------------------------------------------------------------

/// Parse an XML string and return a Document.
#[pyfunction]
fn parse(xml: &str) -> PyResult<Document> {
    Document::new(xml)
}

/// Parse XML bytes and return a Document, with automatic encoding detection.
#[pyfunction]
fn parse_bytes(data: &[u8]) -> PyResult<Document> {
    Document::from_bytes(data)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_write_options(indent: Option<&str>, expand_empty_elements: bool) -> XmlWriteOptions {
    let mut opts = match indent {
        Some(s) => XmlWriteOptions::pretty(s),
        None => XmlWriteOptions::compact(),
    };
    if expand_empty_elements {
        opts = opts.with_expand_empty_elements(true);
    }
    opts
}

// ---------------------------------------------------------------------------
// Module definition
// ---------------------------------------------------------------------------

/// pyuppsala — Python bindings for the Uppsala XML library.
///
/// A zero-dependency XML library providing:
/// - XML 1.0 parsing and well-formedness checking
/// - Namespace-aware DOM with tree mutation
/// - XPath 1.0 evaluation
/// - XSD validation
/// - XSD regex pattern matching
#[pymodule]
fn pyuppsala(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Classes
    m.add_class::<Document>()?;
    m.add_class::<Node>()?;
    m.add_class::<QName>()?;
    m.add_class::<Attribute>()?;
    m.add_class::<XPathEvaluator>()?;
    m.add_class::<XsdValidator>()?;
    m.add_class::<ValidationErrorPy>()?;
    m.add_class::<XmlWriter>()?;
    m.add_class::<XsdRegex>()?;

    // Functions
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(parse_bytes, m)?)?;

    // Exceptions
    m.add("XmlParseError", m.py().get_type::<XmlParseError>())?;
    m.add(
        "XmlWellFormednessError",
        m.py().get_type::<XmlWellFormednessError>(),
    )?;
    m.add("XmlNamespaceError", m.py().get_type::<XmlNamespaceError>())?;
    m.add("XPathError", m.py().get_type::<XPathError>())?;
    m.add(
        "XsdValidationError",
        m.py().get_type::<XsdValidationError>(),
    )?;

    Ok(())
}
