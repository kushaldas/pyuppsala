use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use std::sync::{Arc, Mutex};
use uppsala::dom::{Attribute as UAttribute, NodeId, NodeKind, QName as UQName, XmlWriteOptions};
use uppsala::parser::Parser as UParser;
use uppsala::parser::{DEFAULT_MAX_DEPTH, DEFAULT_MAX_ENTITY_EXPANSION};
use uppsala::writer::XmlWriter as UXmlWriter;
use uppsala::xpath::{XPathEvaluator as UXPathEvaluator, XPathValue as UXPathValue};
use uppsala::xsd::XsdValidator as UXsdValidator;
use uppsala::{Document as UDocument, XmlError};

// ---------------------------------------------------------------------------
// Custom Python exceptions
// ---------------------------------------------------------------------------

create_exception!(_pyuppsala, XmlParseError, pyo3::exceptions::PyException);
create_exception!(
    _pyuppsala,
    XmlWellFormednessError,
    pyo3::exceptions::PyException
);
create_exception!(_pyuppsala, XmlNamespaceError, pyo3::exceptions::PyException);
create_exception!(_pyuppsala, XPathError, pyo3::exceptions::PyException);
create_exception!(
    _pyuppsala,
    XsdValidationError,
    pyo3::exceptions::PyException
);

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
// Shared document handle - allows multiple Python objects to reference one DOM
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
// QName - Python wrapper
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
// Attribute - Python wrapper
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
// Node - a lightweight handle into a Document
// ---------------------------------------------------------------------------

/// A node within an XML document.
///
/// Nodes are lightweight handles - the actual data lives inside the Document.
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

    /// Remove an attribute. Returns the old value if any.
    ///
    /// With `namespace_uri=None` the attribute is matched by local name only
    /// (uppsala's default). When a namespace URI is given, only the attribute
    /// matching both that namespace and local name is removed, so namespaced
    /// attributes sharing a local name are distinguished.
    #[pyo3(signature = (name, namespace_uri=None))]
    fn remove_attribute(
        &self,
        name: &str,
        namespace_uri: Option<&str>,
    ) -> PyResult<Option<String>> {
        let mut guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element_mut(self.id) {
            Some(el) => match namespace_uri {
                None => Ok(el.remove_attribute(name).map(|s| s.to_string())),
                Some(ns) => {
                    let pos = el.attributes.iter().position(|a| {
                        a.name.local_name.as_ref() == name
                            && a.name.namespace_uri.as_deref() == Some(ns)
                    });
                    Ok(pos.map(|i| el.attributes.remove(i).value.into_owned()))
                }
            },
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

    /// A stable integer identity for this node within its Document.
    ///
    /// Two `Node` handles referring to the same underlying node return the same
    /// value. Used by the etree layer to maintain an identity-stable proxy cache.
    #[getter]
    fn node_id(&self) -> usize {
        self.id.index()
    }

    /// The first child node, or None.
    #[getter]
    fn first_child(&self) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.first_child(self.id).map(|cid| Node {
            doc: Arc::clone(&self.doc),
            id: cid,
        }))
    }

    /// The last child node, or None.
    #[getter]
    fn last_child(&self) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.last_child(self.id).map(|cid| Node {
            doc: Arc::clone(&self.doc),
            id: cid,
        }))
    }

    /// The next sibling node, or None.
    #[getter]
    fn next_sibling(&self) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.next_sibling(self.id).map(|sid| Node {
            doc: Arc::clone(&self.doc),
            id: sid,
        }))
    }

    /// The previous sibling node, or None.
    #[getter]
    fn previous_sibling(&self) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.previous_sibling(self.id).map(|sid| Node {
            doc: Arc::clone(&self.doc),
            id: sid,
        }))
    }

    /// In-scope namespace declarations on this element as (prefix, uri) pairs.
    ///
    /// The prefix is None for the default namespace (`xmlns="..."`). Returns an
    /// empty list for non-element nodes.
    #[getter]
    fn namespace_declarations(&self) -> PyResult<Vec<(Option<String>, String)>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element(self.id) {
            Some(el) => Ok(el
                .namespace_declarations
                .iter()
                .map(|(p, u)| {
                    let prefix = if p.is_empty() {
                        None
                    } else {
                        Some(p.to_string())
                    };
                    (prefix, u.to_string())
                })
                .collect()),
            None => Ok(Vec::new()),
        }
    }

    /// Set the content of a Text, CDATA, or Comment node in place.
    ///
    /// Raises ValueError for other node kinds. Used by the etree layer to assign
    /// element `.text`/`.tail` and comment text without recreating nodes.
    fn set_text(&self, content: &str) -> PyResult<()> {
        let mut guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind_mut(self.id) {
            Some(NodeKind::Text(t)) => {
                *t = std::borrow::Cow::Owned(content.to_string());
                Ok(())
            }
            Some(NodeKind::CData(t)) => {
                *t = std::borrow::Cow::Owned(content.to_string());
                Ok(())
            }
            Some(NodeKind::Comment(t)) => {
                *t = std::borrow::Cow::Owned(content.to_string());
                Ok(())
            }
            _ => Err(PyValueError::new_err(
                "Node is not a text, cdata, or comment node",
            )),
        }
    }

    /// The content of a Comment node, or None for other node kinds.
    #[getter]
    fn comment_text(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind(self.id) {
            Some(NodeKind::Comment(t)) => Ok(Some(t.to_string())),
            _ => Ok(None),
        }
    }

    /// The target of a ProcessingInstruction node, or None for other kinds.
    #[getter]
    fn pi_target(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind(self.id) {
            Some(NodeKind::ProcessingInstruction(pi)) => Ok(Some(pi.target.to_string())),
            _ => Ok(None),
        }
    }

    /// The data of a ProcessingInstruction node, or None.
    #[getter]
    fn pi_data(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind(self.id) {
            Some(NodeKind::ProcessingInstruction(pi)) => {
                Ok(pi.data.as_ref().map(|d| d.to_string()))
            }
            _ => Ok(None),
        }
    }

    /// Set the data of a ProcessingInstruction node. Raises ValueError otherwise.
    #[pyo3(signature = (data=None))]
    fn set_pi_data(&self, data: Option<&str>) -> PyResult<()> {
        let mut guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind_mut(self.id) {
            Some(NodeKind::ProcessingInstruction(pi)) => {
                pi.data = data.map(|d| std::borrow::Cow::Owned(d.to_string()));
                Ok(())
            }
            _ => Err(PyValueError::new_err(
                "Node is not a processing instruction",
            )),
        }
    }

    /// Rename an element node's qualified name in place.
    ///
    /// Raises ValueError if the node is not an element. Used by the etree layer
    /// for `element.tag = ...` assignment.
    #[pyo3(signature = (local_name, namespace_uri=None, prefix=None))]
    fn set_qname(
        &self,
        local_name: &str,
        namespace_uri: Option<&str>,
        prefix: Option<&str>,
    ) -> PyResult<()> {
        let mut guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element_mut(self.id) {
            Some(el) => {
                el.name = match (namespace_uri, prefix) {
                    (Some(ns), Some(p)) => {
                        UQName::full(p.to_string(), ns.to_string(), local_name.to_string())
                    }
                    (Some(ns), None) => {
                        UQName::with_namespace(ns.to_string(), local_name.to_string())
                    }
                    _ => UQName::local(local_name.to_string()),
                };
                Ok(())
            }
            None => Err(PyValueError::new_err("Node is not an element")),
        }
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
        Ok(guard.doc.node_range(self.id).map(|r| (r.start, r.end)))
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
// Document - Python wrapper
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
    ///
    /// Optional keyword arguments override uppsala's safe defaults:
    ///
    /// * ``max_depth`` - maximum element nesting depth (default
    ///   ``DEFAULT_MAX_DEPTH``, from ``uppsala::parser``).
    /// * ``max_entity_expansion`` - maximum total bytes from entity expansion
    ///   (default ``DEFAULT_MAX_ENTITY_EXPANSION``, from ``uppsala::parser``).
    /// * ``namespace_aware`` - when False, disables XML namespace processing.
    ///
    /// .. warning::
    ///    Do not source these values from untrusted input. An attacker who
    ///    controls the cap can re-enable the corresponding DoS attack class
    ///    (deep-nesting stack overflow, billion-laughs entity expansion).
    #[new]
    #[pyo3(signature = (xml, *, max_depth=None, max_entity_expansion=None, namespace_aware=None))]
    fn new(
        xml: &str,
        max_depth: Option<u32>,
        max_entity_expansion: Option<usize>,
        namespace_aware: Option<bool>,
    ) -> PyResult<Self> {
        let input = xml.to_string();
        let parser = build_parser(max_depth, max_entity_expansion, namespace_aware);
        let doc = parser.parse(xml).map_err(xml_error_to_pyerr)?.into_static();
        Ok(Document {
            inner: Arc::new(Mutex::new(DocWithInput { doc, input })),
        })
    }

    /// Parse XML from bytes, with automatic encoding detection (UTF-8/UTF-16,
    /// with or without BOM).
    ///
    /// Optional keyword arguments override uppsala's safe defaults. Encoding
    /// auto-detection is applied in all cases - passing ``max_depth``,
    /// ``max_entity_expansion``, or ``namespace_aware`` does not change how
    /// the bytes are decoded, so UTF-16 input keeps working regardless.
    ///
    /// .. warning::
    ///    Do not source the resource-limit kwargs from untrusted input.
    ///    See :class:`Document` for details.
    #[staticmethod]
    #[pyo3(signature = (data, *, max_depth=None, max_entity_expansion=None, namespace_aware=None))]
    fn from_bytes(
        data: &[u8],
        max_depth: Option<u32>,
        max_entity_expansion: Option<usize>,
        namespace_aware: Option<bool>,
    ) -> PyResult<Document> {
        let input = decode_xml_bytes(data)?;
        let parser = build_parser(max_depth, max_entity_expansion, namespace_aware);
        let doc = parser
            .parse(&input)
            .map_err(xml_error_to_pyerr)?
            .into_static();
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

    /// Add or replace an `xmlns` declaration on an element node.
    ///
    /// `prefix=None` sets the default namespace (`xmlns="uri"`); otherwise sets
    /// `xmlns:prefix="uri"`. Used by the etree layer so namespaced trees built
    /// in memory serialize with correct namespace declarations. Raises
    /// ValueError if `node` is not an element.
    #[pyo3(signature = (node, prefix, uri))]
    fn set_namespace_declaration(
        &self,
        node: &Node,
        prefix: Option<&str>,
        uri: &str,
    ) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.element_mut(node.id) {
            Some(el) => {
                let p = prefix.unwrap_or("");
                match el
                    .namespace_declarations
                    .iter_mut()
                    .find(|(existing, _)| existing.as_ref() == p)
                {
                    Some(slot) => slot.1 = std::borrow::Cow::Owned(uri.to_string()),
                    None => el.namespace_declarations.push((
                        std::borrow::Cow::Owned(p.to_string()),
                        std::borrow::Cow::Owned(uri.to_string()),
                    )),
                }
                Ok(())
            }
            None => Err(PyValueError::new_err("Node is not an element")),
        }
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
    /// Create a new XPath evaluator.
    ///
    /// ``max_depth`` overrides the default expression-tree depth cap
    /// (default 32) used to bound recursive parsing of XPath expressions.
    ///
    /// .. warning::
    ///    Do not source ``max_depth`` from untrusted input - an attacker
    ///    who controls the cap can re-enable XPath stack-overflow attacks.
    #[new]
    #[pyo3(signature = (*, max_depth=None))]
    fn new(max_depth: Option<u32>) -> Self {
        let mut inner = UXPathEvaluator::new();
        if let Some(d) = max_depth {
            inner = inner.with_max_depth(d);
        }
        XPathEvaluator { inner }
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
// XmlWriter - imperative XML builder
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
    ///
    /// ``max_depth`` overrides the default group-nesting cap (default 64)
    /// applied to the pattern at compile time.
    ///
    /// .. warning::
    ///    Do not source ``max_depth`` from untrusted input - an attacker
    ///    who controls the cap can re-enable regex compiler stack overflows.
    #[new]
    #[pyo3(signature = (pattern, *, max_depth=None))]
    fn new(pattern: &str, max_depth: Option<u32>) -> PyResult<Self> {
        let inner = match max_depth {
            Some(d) => uppsala::xsd_regex::XsdRegex::compile_with_max_depth(pattern, d),
            None => uppsala::xsd_regex::XsdRegex::compile(pattern),
        }
        .map_err(|e| PyValueError::new_err(format!("Invalid XSD regex: {}", e)))?;
        Ok(XsdRegex {
            inner,
            pattern: pattern.to_string(),
        })
    }

    /// Test whether the input string fully matches the pattern.
    ///
    /// ``max_steps`` overrides the default backtracking-step cap
    /// (default 1,000,000). The matcher returns ``False`` when the cap
    /// is reached, which prevents catastrophic-backtracking ReDoS.
    ///
    /// .. warning::
    ///    Do not source ``max_steps`` from untrusted input - an attacker
    ///    who controls the cap can re-enable polynomial-ReDoS attacks.
    #[pyo3(signature = (input, *, max_steps=None))]
    fn is_match(&self, input: &str, max_steps: Option<usize>) -> bool {
        match max_steps {
            Some(n) => self.inner.is_match_with_max_steps(input, n),
            None => self.inner.is_match(input),
        }
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
///
/// See ``Document.__init__`` for the keyword arguments that override the
/// safe parser defaults.
#[pyfunction]
#[pyo3(signature = (xml, *, max_depth=None, max_entity_expansion=None, namespace_aware=None))]
fn parse(
    xml: &str,
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
) -> PyResult<Document> {
    Document::new(xml, max_depth, max_entity_expansion, namespace_aware)
}

/// Parse XML bytes and return a Document, with automatic encoding detection.
///
/// See ``Document.from_bytes`` for the keyword arguments that override
/// the safe parser defaults.
#[pyfunction]
#[pyo3(signature = (data, *, max_depth=None, max_entity_expansion=None, namespace_aware=None))]
fn parse_bytes(
    data: &[u8],
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
) -> PyResult<Document> {
    Document::from_bytes(data, max_depth, max_entity_expansion, namespace_aware)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_parser(
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
) -> UParser {
    let mut parser = match namespace_aware {
        Some(false) => UParser::with_namespace_aware(false),
        _ => UParser::new(),
    };
    if let Some(d) = max_depth {
        parser = parser.with_max_depth(d);
    }
    if let Some(b) = max_entity_expansion {
        parser = parser.with_max_entity_expansion(b);
    }
    parser
}

/// Decode raw XML bytes to a String, auto-detecting the encoding (UTF-8 and
/// UTF-16 LE/BE, with or without BOM). This mirrors uppsala's internal
/// `decode_xml_bytes` so the keyword-argument code path keeps the same
/// encoding support as the plain `parse_bytes` fast path - the `Parser`
/// builder only accepts `&str`, so without this the only option would be a
/// lossy UTF-8 decode that mangles UTF-16 input.
fn decode_xml_bytes(data: &[u8]) -> PyResult<String> {
    if data.len() < 2 {
        // Too short for BOM detection - assume UTF-8.
        return decode_utf8(data);
    }

    // Byte-order mark detection.
    if data[0] == 0xFF && data[1] == 0xFE {
        return decode_utf16(&data[2..], false); // UTF-16 LE BOM
    }
    if data[0] == 0xFE && data[1] == 0xFF {
        return decode_utf16(&data[2..], true); // UTF-16 BE BOM
    }
    if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        // UTF-8 BOM - strip it and decode as UTF-8.
        return decode_utf8(&data[3..]);
    }

    // No BOM - check for UTF-16 without BOM (XML spec Appendix F).
    if data[0] == 0x00 && data[1] == 0x3C {
        return decode_utf16(data, true); // UTF-16 BE without BOM
    }
    if data[0] == 0x3C && data[1] == 0x00 {
        return decode_utf16(data, false); // UTF-16 LE without BOM
    }

    // Default: UTF-8.
    decode_utf8(data)
}

/// Validate UTF-8 bytes and copy them into a String. Borrows the slice for
/// validation (`std::str::from_utf8`) so there is no intermediate `Vec<u8>`
/// allocation on the common UTF-8 path - only the final owned copy.
fn decode_utf8(bytes: &[u8]) -> PyResult<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|e| XmlWellFormednessError::new_err(format!("1:1: Invalid UTF-8: {}", e)))
}

/// Decode UTF-16 bytes (big- or little-endian) to a String. An odd-length
/// input is rejected as malformed rather than silently dropping the trailing
/// byte (which could truncate invalid UTF-16 into superficially valid text).
fn decode_utf16(bytes: &[u8], big_endian: bool) -> PyResult<String> {
    let endian = if big_endian { "BE" } else { "LE" };
    if !bytes.len().is_multiple_of(2) {
        return Err(XmlWellFormednessError::new_err(format!(
            "1:1: Invalid UTF-16 {}: odd number of bytes",
            endian
        )));
    }
    let code_units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| {
            if big_endian {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_le_bytes([chunk[0], chunk[1]])
            }
        })
        .collect();
    String::from_utf16(&code_units).map_err(|e| {
        XmlWellFormednessError::new_err(format!("1:1: Invalid UTF-16 {}: {}", endian, e))
    })
}

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

/// pyuppsala - Python bindings for the Uppsala XML library.
///
/// A zero-dependency XML library providing:
/// - XML 1.0 parsing and well-formedness checking
/// - Namespace-aware DOM with tree mutation
/// - XPath 1.0 evaluation
/// - XSD validation
/// - XSD regex pattern matching
#[pymodule]
fn _pyuppsala(m: &Bound<'_, PyModule>) -> PyResult<()> {
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

    // Default resource-limit constants (uppsala 0.4.0 hardening)
    m.add("DEFAULT_MAX_DEPTH", DEFAULT_MAX_DEPTH)?;
    m.add("DEFAULT_MAX_ENTITY_EXPANSION", DEFAULT_MAX_ENTITY_EXPANSION)?;
    m.add(
        "DEFAULT_MAX_XPATH_DEPTH",
        uppsala::xpath::DEFAULT_MAX_XPATH_DEPTH,
    )?;
    m.add(
        "DEFAULT_MAX_REGEX_GROUP_DEPTH",
        uppsala::xsd_regex::DEFAULT_MAX_REGEX_GROUP_DEPTH,
    )?;
    m.add(
        "DEFAULT_MAX_REGEX_STEPS",
        uppsala::xsd_regex::DEFAULT_MAX_REGEX_STEPS,
    )?;

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
