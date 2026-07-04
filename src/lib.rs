use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyDict;

use std::sync::{Arc, Mutex};
use uppsala::dom::{Attribute as UAttribute, NodeId, NodeKind, QName as UQName, XmlWriteOptions};
use uppsala::parser::Parser as UParser;
use uppsala::parser::{DEFAULT_MAX_DEPTH, DEFAULT_MAX_ENTITY_DEPTH, DEFAULT_MAX_ENTITY_EXPANSION};
use uppsala::writer::XmlWriter as UXmlWriter;
use uppsala::xpath::{XPathEvaluator as UXPathEvaluator, XPathValue as UXPathValue};
use uppsala::xsd::XsdValidator as UXsdValidator;
use uppsala::{Document as UDocument, XmlError};

// ---------------------------------------------------------------------------
// Custom Python exceptions
// ---------------------------------------------------------------------------

// The module name passed to `create_exception!` becomes each exception's
// Python `__module__`. Use the public package `pyuppsala` (which re-exports
// these exceptions) rather than the internal `_pyuppsala` extension: there is
// no importable top-level `_pyuppsala` module, so the latter breaks pickling
// and produces misleading tracebacks. With `pyuppsala`, `__module__` resolves
// via the package's re-exports.
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

fn is_xml_name_start(c: char) -> bool {
    // The exact XML 1.0 NameStartChar production. The ranges are kept literal
    // (rather than a single 0xC0..=0xD7FF span) so disallowed code points such
    // as U+00D7, U+00F7, and the combining marks at U+0300..U+036F are excluded.
    let u = c as u32;
    c == ':'
        || c == '_'
        || c.is_ascii_alphabetic()
        || (0x00C0..=0x00D6).contains(&u)
        || (0x00D8..=0x00F6).contains(&u)
        || (0x00F8..=0x02FF).contains(&u)
        || (0x0370..=0x037D).contains(&u)
        || (0x037F..=0x1FFF).contains(&u)
        || (0x200C..=0x200D).contains(&u)
        || (0x2070..=0x218F).contains(&u)
        || (0x2C00..=0x2FEF).contains(&u)
        || (0x3001..=0xD7FF).contains(&u)
        || (0xF900..=0xFDCF).contains(&u)
        || (0xFDF0..=0xFFFD).contains(&u)
        || (0x10000..=0xEFFFF).contains(&u)
}

fn is_xml_name_char(c: char) -> bool {
    let u = c as u32;
    is_xml_name_start(c)
        || c == '-'
        || c == '.'
        || c.is_ascii_digit()
        || c == '\u{00B7}'
        || (0x0300..=0x036F).contains(&u)
        || (0x203F..=0x2040).contains(&u)
}

fn validate_name_with<F, G>(value: &str, what: &str, start_ok: F, char_ok: G) -> PyResult<()>
where
    F: Fn(char) -> bool,
    G: Fn(char) -> bool,
{
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if start_ok(first) && chars.all(char_ok) => Ok(()),
        _ => Err(PyValueError::new_err(format!(
            "Invalid {} name: {:?}",
            what, value
        ))),
    }
}

fn validate_xml_name(value: &str, what: &str) -> PyResult<()> {
    // The streaming writer accepts already-prefixed names like "x:item", so it
    // validates XML Name rather than NCName.
    validate_name_with(value, what, is_xml_name_start, is_xml_name_char)
}

fn validate_ncname(value: &str, what: &str) -> PyResult<()> {
    // DOM builders receive local names and prefixes separately.  A colon inside
    // either one would be ambiguous and can produce malformed serialized XML.
    validate_name_with(
        value,
        what,
        |c| c != ':' && is_xml_name_start(c),
        |c| c != ':' && is_xml_name_char(c),
    )
}

fn validate_prefix(prefix: Option<&str>) -> PyResult<Option<&str>> {
    match prefix {
        Some("") | None => Ok(None),
        Some(p) => {
            validate_ncname(p, "namespace prefix")?;
            Ok(Some(p))
        }
    }
}

// The two namespaces the XML Namespaces spec reserves and binds to fixed
// prefixes. They guard against rebinding `xml`/`xmlns` or declaring them in
// ways that would produce invalid XML.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// Validate a `(namespace_uri, prefix)` pair used to build an element or
/// attribute QName.
///
/// Returns the normalized prefix (empty string mapped to `None`). A prefix is
/// only meaningful alongside a namespace URI, so a prefix supplied without a
/// namespace is rejected rather than being silently dropped. The XML
/// Namespaces reserved bindings are enforced too, so a QName cannot use the
/// `xmlns` prefix, rebind the `xml` prefix or XML namespace, or sit in the
/// `xmlns` namespace - all of which would serialize to invalid XML.
fn validate_qname_parts<'a>(
    namespace_uri: Option<&str>,
    prefix: Option<&'a str>,
) -> PyResult<Option<&'a str>> {
    let prefix = validate_prefix(prefix)?;
    match namespace_uri {
        None => {
            if prefix.is_some() {
                return Err(PyValueError::new_err(
                    "a namespace prefix requires a namespace URI",
                ));
            }
        }
        Some(ns) => {
            if prefix == Some("xmlns") {
                return Err(PyValueError::new_err(
                    "the \"xmlns\" prefix is reserved and cannot be used as a name prefix",
                ));
            }
            if ns == XMLNS_NAMESPACE {
                return Err(PyValueError::new_err(
                    "the xmlns namespace cannot be used for element or attribute names",
                ));
            }
            if prefix == Some("xml") && ns != XML_NAMESPACE {
                return Err(PyValueError::new_err(
                    "the \"xml\" prefix can only be bound to the XML namespace",
                ));
            }
            if ns == XML_NAMESPACE && prefix != Some("xml") {
                return Err(PyValueError::new_err(
                    "the XML namespace can only be used with the \"xml\" prefix",
                ));
            }
        }
    }
    Ok(prefix)
}

/// Reject `xmlns` declarations that the XML Namespaces spec forbids: the
/// reserved `xmlns` prefix, rebinding the `xml` prefix or XML namespace to
/// anything else, and declaring the `xmlns` namespace at all. These would
/// otherwise serialize to invalid XML or clobber the standard `xml` binding.
fn validate_ns_declaration(prefix: Option<&str>, uri: &str) -> PyResult<()> {
    if prefix == Some("xmlns") {
        return Err(PyValueError::new_err(
            "the \"xmlns\" prefix is reserved and cannot be declared",
        ));
    }
    if prefix == Some("xml") && uri != XML_NAMESPACE {
        return Err(PyValueError::new_err(
            "the \"xml\" prefix can only be bound to the XML namespace",
        ));
    }
    if uri == XML_NAMESPACE && prefix != Some("xml") {
        return Err(PyValueError::new_err(
            "the XML namespace can only be bound to the \"xml\" prefix",
        ));
    }
    if uri == XMLNS_NAMESPACE {
        return Err(PyValueError::new_err(
            "the xmlns namespace cannot be declared",
        ));
    }
    Ok(())
}

fn validate_pi_target(target: &str) -> PyResult<()> {
    validate_xml_name(target, "processing instruction target")?;
    if target.eq_ignore_ascii_case("xml") {
        return Err(PyValueError::new_err(
            "Invalid processing instruction target: reserved XML target",
        ));
    }
    Ok(())
}

fn writer_attr_refs(attrs: &Option<Vec<(String, String)>>) -> PyResult<Vec<(&str, &str)>> {
    // Attribute values are escaped by the writer. Attribute names are not, so
    // validate them before handing references to the underlying writer.
    match attrs {
        Some(a) => {
            for (name, _) in a {
                validate_xml_name(name, "attribute")?;
            }
            Ok(a.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect())
        }
        None => Ok(Vec::new()),
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

/// Ensure a Python ``Node`` handle belongs to the receiver ``Document``.
///
/// Uppsala ``NodeId`` values are scoped to one document, but the Python binding
/// exposes nodes as independent handles. Same-document mutators must reject a
/// handle from another document before passing its numeric id into the receiver
/// document, otherwise a colliding id could select and mutate the wrong node.
fn ensure_node_in_document(doc: &SharedDoc, node: &Node, role: &str) -> PyResult<()> {
    if Arc::ptr_eq(doc, &node.doc) {
        Ok(())
    } else {
        Err(PyValueError::new_err(format!(
            "{} belongs to a different Document",
            role
        )))
    }
}

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
    fn new(
        local_name: String,
        namespace_uri: Option<String>,
        prefix: Option<String>,
    ) -> PyResult<Self> {
        validate_ncname(&local_name, "local")?;
        // Enforce the same QName invariants as the DOM builders (a prefix
        // requires a namespace URI, plus the reserved xml/xmlns bindings) and
        // normalize an empty prefix to None, so a QName can never represent a
        // name that create_element/set_attribute/set_qname would reject.
        let prefix =
            validate_qname_parts(namespace_uri.as_deref(), prefix.as_deref())?.map(str::to_string);
        Ok(QName {
            namespace_uri,
            prefix,
            local_name,
        })
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
// freelist: Node shells are the churn object of the binding (every
// navigation step creates a short-lived (Arc, NodeId) pair); pooling frees
// them without malloc/free round-trips. Node is final (not subclassable), so
// the freelist always applies. Bench-gated -- see benchmarks/etree_bench.py.
#[pyclass(name = "Node", from_py_object, freelist = 2048)]
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

    /// The element's tag in Clark `{uri}local` notation, built natively, or
    /// None for non-element nodes.
    ///
    /// The etree `.tag` property is extremely hot (pyFF reads it per element
    /// while scanning the tree). Returning the Clark string directly avoids
    /// allocating an intermediate `QName` Python object and rebuilding the
    /// string in Python on every access; a `None` result lets the caller fall
    /// back to the comment/PI handling. An absent or empty namespace yields a
    /// bare local name, matching lxml's no-namespace convention.
    fn clark_tag(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.element(self.id).map(|el| {
            let q = &el.name;
            match &q.namespace_uri {
                Some(ns) if !ns.is_empty() => format!("{{{}}}{}", ns, q.local_name),
                _ => q.local_name.to_string(),
            }
        }))
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

    /// For attribute nodes (e.g. from an XPath ``@name`` / attribute-axis
    /// selection), the attribute's string value; ``None`` for every other node
    /// kind. The etree layer uses this to return attribute values as plain
    /// strings, matching lxml's ``xpath("...//@attr")``.
    #[getter]
    fn attribute_value(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        match guard.doc.node_kind(self.id) {
            Some(NodeKind::Attribute(_, value)) => Ok(Some(value.to_string())),
            _ => Ok(None),
        }
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
        validate_ncname(name, "attribute")?;
        let prefix = validate_qname_parts(namespace_uri, prefix)?;
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
    /// `namespace_uri=None` removes the attribute that has *no* namespace and
    /// the given local name; a namespace URI removes the attribute in exactly
    /// that namespace. In both cases an attribute in a different namespace that
    /// merely shares the local name is left untouched.
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
            Some(el) => {
                let pos = el.attributes.iter().position(|a| {
                    a.name.local_name.as_ref() == name
                        && a.name.namespace_uri.as_deref() == namespace_uri
                });
                Ok(pos.map(|i| el.attributes.remove(i).value.into_owned()))
            }
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

    /// The children lxml treats as element content: elements, comments and
    /// processing instructions, in document order. Text and CDATA children are
    /// excluded because lxml exposes those via `.text`/`.tail` rather than as
    /// indexable children.
    ///
    /// Filtered natively under a single lock (walking the sibling chain), versus
    /// the etree layer otherwise materialising every child and querying each
    /// one's kind over FFI. This is hot: pyFF's whole-tree visits (`list(elt)`
    /// recursion) hit it once per element.
    fn content_children(&self) -> PyResult<Vec<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut out = Vec::new();
        let mut child = guard.doc.first_child(self.id);
        while let Some(cid) = child {
            if matches!(
                guard.doc.node_kind(cid),
                Some(NodeKind::Element(_))
                    | Some(NodeKind::Comment(_))
                    | Some(NodeKind::ProcessingInstruction(_))
            ) {
                out.push(Node {
                    doc: Arc::clone(&self.doc),
                    id: cid,
                });
            }
            child = guard.doc.next_sibling(cid);
        }
        Ok(out)
    }

    /// The number of content children (elements, comments and processing
    /// instructions), counted natively without materialising any `Node`.
    ///
    /// Backs the etree layer's `_Element.__len__`. `list(elt)` asks for the
    /// length as a sizing hint *and* then iterates, so a plain
    /// `len(content_children())` built and threw away a whole `Vec<Node>` on the
    /// length call alone; counting in place avoids that allocation on the hot
    /// whole-tree-visit path.
    fn content_child_count(&self) -> PyResult<usize> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut n = 0usize;
        let mut child = guard.doc.first_child(self.id);
        while let Some(cid) = child {
            if matches!(
                guard.doc.node_kind(cid),
                Some(NodeKind::Element(_))
                    | Some(NodeKind::Comment(_))
                    | Some(NodeKind::ProcessingInstruction(_))
            ) {
                n += 1;
            }
            child = guard.doc.next_sibling(cid);
        }
        Ok(n)
    }

    /// The element's leading text/CDATA run as a single string (etree `.text`).
    ///
    /// ElementTree exposes `.text` as the contiguous run of Text/CDATA nodes that
    /// starts at the first child; with `strip_cdata=False` that run can mix Text
    /// and CDATA nodes, which the public string concatenates. Returns `None` when
    /// the first child is not a text node (no leading text), matching lxml.
    /// Walks the whole run under a single lock, versus the Python layer's per-node
    /// `first_child`/`kind`/`text`/`next_sibling` FFI calls.
    fn leading_text_run(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut out: Option<String> = None;
        let mut child = guard.doc.first_child(self.id);
        while let Some(cid) = child {
            match guard.doc.node_kind(cid) {
                Some(NodeKind::Text(_)) | Some(NodeKind::CData(_)) => {
                    let s = guard.doc.text_content(cid).unwrap_or("");
                    out.get_or_insert_with(String::new).push_str(s);
                    child = guard.doc.next_sibling(cid);
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// The element's trailing text/CDATA run as a single string (etree `.tail`).
    ///
    /// The contiguous run of Text/CDATA nodes that starts at this node's next
    /// sibling, concatenated; `None` when the next sibling is not a text node.
    /// Single-lock equivalent of the Python `_following_text_run` + `_run_text`.
    fn tail_text_run(&self) -> PyResult<Option<String>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut out: Option<String> = None;
        let mut sib = guard.doc.next_sibling(self.id);
        while let Some(sid) = sib {
            match guard.doc.node_kind(sid) {
                Some(NodeKind::Text(_)) | Some(NodeKind::CData(_)) => {
                    let s = guard.doc.text_content(sid).unwrap_or("");
                    out.get_or_insert_with(String::new).push_str(s);
                    sib = guard.doc.next_sibling(sid);
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// A stable integer identity for this node within its Document.
    ///
    /// Two `Node` handles referring to the same underlying node return the same
    /// value. Used by the etree layer to maintain an identity-stable proxy cache.
    #[getter]
    fn node_id(&self) -> usize {
        self.id.index()
    }

    /// Return a lazy pre-order descendant iterator over this node and its
    /// subtree, optionally filtered by tag, matching lxml's ``Element.iter``.
    ///
    /// The whole pre-order tree walk and tag matching run natively (one mutex
    /// acquisition per ``__next__``, not per visited node), so the Python etree
    /// layer only pays a proxy-wrap cost for the nodes that actually match
    /// rather than walking every node in Python. This is the hot path that
    /// dominated pyFF (see pyFF/performance.md): the aggregate has tens of
    /// thousands of nodes and pyFF iterates it repeatedly.
    ///
    /// ``tag`` semantics follow lxml / ElementTree:
    ///
    /// * ``None`` yields elements, comments and processing instructions;
    /// * ``"*"`` yields elements only;
    /// * a Clark-notation name (``"{ns}local"`` or ``"local"``) yields only
    ///   matching elements. An empty namespace (``"{}local"``) and a bare local
    ///   name both match elements that have no namespace.
    ///
    /// The starting node itself is included when it qualifies (lxml includes
    /// the context element in ``iter``).
    fn iter_descendants(&self, tag: Option<&str>) -> DescendantIterator {
        DescendantIterator {
            doc: Arc::clone(&self.doc),
            stack: vec![self.id],
            filter: DescFilter::parse(tag),
        }
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

    /// The namespace declarations on this element as (prefix, uri) pairs.
    ///
    /// Only the `xmlns`/`xmlns:*` declarations attached to this element itself
    /// are returned, not declarations inherited from ancestors. The prefix is
    /// None for the default namespace (`xmlns="..."`). Returns an empty list for
    /// non-element nodes.
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

    /// In-scope namespace declarations for this element, as `(prefix, uri)`
    /// pairs ordered outermost (root) first, so `dict(...)` of the result yields
    /// inner declarations overriding outer ones. `prefix` is `None` for the
    /// default namespace, matching lxml's `Element.nsmap` key convention.
    ///
    /// Walks this element and its ancestors in a single native pass (one lock)
    /// rather than one FFI call per ancestor per declaration, which the etree
    /// layer's `nsmap` property otherwise pays once per element when pyFF scans
    /// the whole tree.
    fn nsmap(&self) -> PyResult<Vec<(Option<String>, String)>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        // Collect this element and its ancestor elements, innermost first.
        let mut chain: Vec<NodeId> = Vec::new();
        let mut cur = Some(self.id);
        while let Some(id) = cur {
            match guard.doc.node_kind(id) {
                Some(NodeKind::Element(_)) => {
                    chain.push(id);
                    cur = guard.doc.parent(id);
                }
                _ => break,
            }
        }
        // Emit outermost first so a later (inner) entry wins under `dict(...)`.
        let mut pairs = Vec::new();
        for &id in chain.iter().rev() {
            if let Some(NodeKind::Element(e)) = guard.doc.node_kind(id) {
                for (p, u) in &e.namespace_declarations {
                    let prefix = if p.is_empty() {
                        None
                    } else {
                        Some(p.to_string())
                    };
                    pairs.push((prefix, u.to_string()));
                }
            }
        }
        Ok(pairs)
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
        validate_ncname(local_name, "element")?;
        let prefix = validate_qname_parts(namespace_uri, prefix)?;
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
    fn to_xml(&self, py: Python<'_>) -> PyResult<String> {
        // Subtree serialization is pure Rust producing a String; run it
        // detached (GIL released), locking the doc inside the closure.
        let shared = Arc::clone(&self.doc);
        let id = self.id;
        py.detach(|| {
            let guard = shared.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(guard.doc.node_to_xml(id))
        })
        .map_err(PyRuntimeError::new_err)
    }

    /// Serialize this node and its subtree to XML with formatting options.
    #[pyo3(signature = (indent=None, expand_empty_elements=false))]
    fn to_xml_with_options(
        &self,
        py: Python<'_>,
        indent: Option<&str>,
        expand_empty_elements: bool,
    ) -> PyResult<String> {
        // Node-level (fragment) serialization never emits a DOCTYPE, so
        // `include_doctype` is fixed to false here. DOCTYPE round-tripping is
        // only meaningful for whole-document serialization (see
        // `Document.to_xml_with_options`).
        let opts = make_write_options(indent, expand_empty_elements, false);
        let shared = Arc::clone(&self.doc);
        let id = self.id;
        py.detach(|| {
            let guard = shared.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(guard.doc.node_to_xml_with_options(id, &opts))
        })
        .map_err(PyRuntimeError::new_err)
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

    fn __str__(&self, py: Python<'_>) -> PyResult<String> {
        self.to_xml(py)
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
// ElementBase -- native base class for the etree `_Element`
// ---------------------------------------------------------------------------

/// The etree `Comment` / `ProcessingInstruction` factory callables and the
/// Python tag-setter helper, registered once from `etree.py` at import time via
/// `_register_element_helpers`. The native `.tag` getter must return the *exact*
/// `Comment` / `ProcessingInstruction` objects for comment/PI nodes so that
/// `elem.tag is Comment` holds (lxml compatibility), and the `.tag` setter
/// delegates the (cold) namespace-finalisation logic back to Python rather than
/// re-implementing `_finalize_element_ns` / `_prefix_for_ns` in Rust.
static COMMENT_FACTORY: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static PI_FACTORY: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SET_TAG_CB: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SET_TEXT_CB: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SET_TAIL_CB: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Register the etree helper callables the native `ElementBase` needs. Called
/// once from `pyuppsala.etree` at import; subsequent calls are ignored. The
/// `set_*` callbacks carry the cold, mutation-heavy property setters (renaming,
/// text/tail replacement) that stay in Python; the matching getters are native.
#[pyfunction]
fn _register_element_helpers(
    py: Python<'_>,
    comment: Py<PyAny>,
    processing_instruction: Py<PyAny>,
    set_tag: Py<PyAny>,
    set_text: Py<PyAny>,
    set_tail: Py<PyAny>,
) -> PyResult<()> {
    let _ = COMMENT_FACTORY.set(py, comment);
    let _ = PI_FACTORY.set(py, processing_instruction);
    let _ = SET_TAG_CB.set(py, set_tag);
    let _ = SET_TEXT_CB.set(py, set_text);
    let _ = SET_TAIL_CB.set(py, set_tail);
    Ok(())
}

/// Invoke a registered Python setter callback `cb(self, value)`; used by the
/// native text/tail/tag setters to delegate the cold mutation logic to Python.
fn call_setter(
    cell: &PyOnceLock<Py<PyAny>>,
    slf: Bound<'_, ElementBase>,
    value: Bound<'_, PyAny>,
) -> PyResult<()> {
    let py = slf.py();
    let cb = cell
        .get(py)
        .ok_or_else(|| PyRuntimeError::new_err("etree element helpers not registered"))?;
    cb.call1(py, (slf, value))?;
    Ok(())
}

/// Return an owned clone of a registered factory object (the `Comment` /
/// `ProcessingInstruction` callables). Fails fast with the same error as
/// `call_setter` if the etree helpers were never registered, rather than
/// silently returning `None` and masking an import/initialization bug.
fn registered_factory(py: Python<'_>, cell: &PyOnceLock<Py<PyAny>>) -> PyResult<Py<PyAny>> {
    cell.get(py)
        .map(|f| f.clone_ref(py))
        .ok_or_else(|| PyRuntimeError::new_err("etree element helpers not registered"))
}

/// The etree `_Element` type object, registered once from `etree.py` right
/// after the class definition. The native proxy cache (`DocHolderBase`)
/// constructs `_Element` instances directly through this type object on a
/// cache miss -- `type.__call__` runs only the PyO3-generated `tp_new`
/// (`ElementBase::new`), since `_Element` defines no `__init__`, so no Python
/// frame executes on the miss path and none at all on a hit.
static ELEMENT_TYPE: PyOnceLock<Py<pyo3::types::PyType>> = PyOnceLock::new();

/// Register the concrete etree `_Element` class used by the native proxy
/// cache. Called once from `pyuppsala.etree` at import; later calls are
/// ignored (first registration wins, matching `_register_element_helpers`).
#[pyfunction]
fn _register_element_type(py: Python<'_>, element_type: Py<pyo3::types::PyType>) -> PyResult<()> {
    let _ = ELEMENT_TYPE.set(py, element_type);
    Ok(())
}

/// Subclassable native base for `pyuppsala.etree._Element`.
///
/// The etree layer's `_Element` is a live, identity-stable view over a node in
/// a native `Document`. Historically it was a pure-Python class holding three
/// slots (`_holder`, `_node`, `_id`); this base lets the hot methods move into
/// Rust one area at a time while the Python `_Element` subclass keeps the colder
/// methods (mutation, serialization, find, xinclude). Because every proxy is the
/// same type and shares one per-document cache, the move must be all-or-nothing
/// at the *type* level, hence a subclassable base rather than a parallel class.
///
/// State mirrors the old slots and is exposed under the same names (`_holder`,
/// `_node`, `_id`) as read/write properties, so the existing Python methods and
/// the cross-tree `_repoint_subtree` keep working unchanged: the cache still
/// refreshes `_node` after mutations and repoint still re-points `_holder`/
/// `_node`/`_id`. `weakref` is enabled so the per-document proxy cache can hold
/// callback-free `weakref.ref`s to these instances.
#[pyclass(subclass, weakref, name = "_ElementBase")]
struct ElementBase {
    /// The Python `_DocHolder` that owns the document and the proxy cache.
    holder: Py<PyAny>,
    /// The native `Node` handle this proxy currently points at.
    node: Py<Node>,
    /// The node's stable per-document id (mirrors `Node.node_id`).
    node_id: usize,
}

#[pymethods]
impl ElementBase {
    #[new]
    fn new(holder: Py<PyAny>, node: Py<Node>, node_id: usize) -> Self {
        ElementBase {
            holder,
            node,
            node_id,
        }
    }

    #[getter(_holder)]
    fn get_holder(&self, py: Python<'_>) -> Py<PyAny> {
        self.holder.clone_ref(py)
    }

    #[setter(_holder)]
    fn set_holder(&mut self, value: Py<PyAny>) {
        self.holder = value;
    }

    #[getter(_node)]
    fn get_node(&self, py: Python<'_>) -> Py<Node> {
        self.node.clone_ref(py)
    }

    #[setter(_node)]
    fn set_node(&mut self, value: Py<Node>) {
        self.node = value;
    }

    #[getter(_id)]
    fn get_id(&self) -> usize {
        self.node_id
    }

    #[setter(_id)]
    fn set_id(&mut self, value: usize) {
        self.node_id = value;
    }

    /// The number of child elements (and comments/PIs) -- etree `__len__`.
    ///
    /// Counts natively without materialising the child list (see
    /// `Node.content_child_count`), since `list(elt)` asks for the length as a
    /// sizing hint before iterating.
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        self.node.bind(py).borrow().content_child_count()
    }

    /// The element's tag in Clark `{uri}local` notation.
    ///
    /// This is the single hottest getter in the etree layer (read once per node
    /// on every whole-tree walk), so the element case is a single native call
    /// returning the Clark string directly, with no Python frame and no
    /// intermediate `QName`. Comment and processing-instruction nodes return the
    /// `Comment` / `ProcessingInstruction` factory (so `elem.tag is Comment`
    /// identifies a comment, matching lxml); any other kind returns `None`.
    #[getter(tag)]
    fn get_tag(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let node_ref = self.node.bind(py).borrow();
        // Element fast path: return the interned Clark string from the
        // holder's tag table -- the same Py<PyString> for every element with
        // this qualified name, so repeated .tag reads allocate nothing and
        // equal tags compare by pointer identity first.
        //
        // The document mutex is held only while *reading* the name: a
        // tag-table hit resolves under it too (hash lookup + piecewise
        // compare + incref, no Python-object allocation), but on a miss the
        // name is copied out and the lock released before any Python string
        // is created -- allocating can trigger GC/finalizers that re-enter
        // this document, which must not happen with the lock held.
        let missed: Option<(Option<String>, String)>;
        {
            let guard = node_ref
                .doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            if let Some(e) = guard.doc.element(node_ref.id) {
                let ns = e.name.namespace_uri.as_deref();
                let local = &e.name.local_name;
                if let Ok(holder) = self.holder.bind(py).cast::<DocHolderBase>() {
                    if let Some(s) = holder.borrow().lookup_tag(py, ns, local)? {
                        return Ok(s.into_any());
                    }
                }
                missed = Some((ns.map(str::to_owned), local.to_string()));
            } else {
                missed = None;
            }
        }
        // Tag-table miss (or unusual holder): the lock is released, so it is
        // now safe to allocate Python objects.
        if let Some((ns, local)) = missed {
            if let Ok(holder) = self.holder.bind(py).cast::<DocHolderBase>() {
                let interned = DocHolderBase::intern_tag(holder, py, ns.as_deref(), &local)?;
                return Ok(interned.into_any());
            }
            // Unusual holder (not a _DocHolder): fall back to building the
            // Clark string fresh, preserving old behaviour.
            let clark = match ns {
                Some(ns) => format!("{{{}}}{}", ns, local),
                None => local,
            };
            return Ok(clark.into_pyobject(py)?.into_any().unbind());
        }
        let kind = node_ref.kind()?;
        match kind.as_str() {
            "comment" => registered_factory(py, &COMMENT_FACTORY),
            "processing_instruction" => registered_factory(py, &PI_FACTORY),
            _ => Ok(py.None()),
        }
    }

    /// Rename the element, keeping its namespace declared/in scope.
    ///
    /// The namespace-finalisation logic (reuse an in-scope binding for the new
    /// URI, else declare/generate a prefix) is cold and intricate, so it is left
    /// in Python: this setter forwards the live element and the new value to the
    /// registered `_set_element_tag` callback.
    #[setter(tag)]
    fn set_tag(slf: Bound<'_, Self>, value: Bound<'_, PyAny>) -> PyResult<()> {
        call_setter(&SET_TAG_CB, slf, value)
    }

    /// The text directly inside this element, before its first child, or `None`.
    ///
    /// For comment / processing-instruction nodes this is the comment / PI body
    /// instead, matching lxml. For elements it is the leading Text/CDATA run (see
    /// `Node.leading_text_run`).
    #[getter(text)]
    fn get_text(&self, py: Python<'_>) -> PyResult<Option<String>> {
        let node_ref = self.node.bind(py).borrow();
        match node_ref.kind()?.as_str() {
            "comment" => node_ref.comment_text(),
            "processing_instruction" => node_ref.pi_data(),
            _ => node_ref.leading_text_run(),
        }
    }

    /// Set leading text (or the comment/PI body), replacing the existing run.
    /// The mutation logic is cold and intricate, so it stays in Python.
    #[setter(text)]
    fn set_text(slf: Bound<'_, Self>, value: Bound<'_, PyAny>) -> PyResult<()> {
        call_setter(&SET_TEXT_CB, slf, value)
    }

    /// The text following this element's end tag, before the next sibling, or
    /// `None` -- the trailing Text/CDATA run (see `Node.tail_text_run`).
    #[getter(tail)]
    fn get_tail(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.node.bind(py).borrow().tail_text_run()
    }

    /// Set trailing text, replacing the existing run. Cold; stays in Python.
    #[setter(tail)]
    fn set_tail(slf: Bound<'_, Self>, value: Bound<'_, PyAny>) -> PyResult<()> {
        call_setter(&SET_TAIL_CB, slf, value)
    }

    /// Mapping of in-scope prefixes to URIs (None key = default namespace).
    ///
    /// The ancestor walk and declaration collection run natively in a single lock
    /// (`Node.nsmap`), returning pairs outermost-first; building the dict here in
    /// Rust keeps the inner (later) binding per prefix, matching lxml, and avoids
    /// the Python property frame plus the `dict(...)` call.
    #[getter(nsmap)]
    fn get_nsmap(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let pairs = self.node.bind(py).borrow().nsmap()?;
        let d = PyDict::new(py);
        for (prefix, uri) in pairs {
            d.set_item(prefix, uri)?;
        }
        Ok(d.unbind())
    }

    /// The namespace prefix of this element's tag, or None.
    #[getter(prefix)]
    fn get_prefix(&self, py: Python<'_>) -> PyResult<Option<String>> {
        let q = self.node.bind(py).borrow().tag()?;
        Ok(q.and_then(|qn| qn.prefix().map(|s| s.to_string())))
    }

    /// The 1-based source line of this element, or None for built nodes.
    #[getter(sourceline)]
    fn get_sourceline(&self, py: Python<'_>) -> PyResult<Option<usize>> {
        match self.node.bind(py).borrow().line() {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(n)),
            Err(_) => Ok(None),
        }
    }

    /// Native backing of `_Element.iter`: a pre-order descendant iterator
    /// (including self) that yields identity-stable proxies straight from the
    /// holder's cache. `tag` follows lxml semantics (`None` = everything,
    /// `"*"` = elements only, Clark string / bare name = matching elements);
    /// QName normalisation stays in the Python wrapper.
    fn _iter_proxies(
        &self,
        py: Python<'_>,
        tag: Option<&str>,
    ) -> PyResult<ProxyDescendantIterator> {
        let holder: Py<DocHolderBase> = self
            .holder
            .bind(py)
            .cast::<DocHolderBase>()?
            .clone()
            .unbind();
        let node = self.node.bind(py).borrow();
        Ok(ProxyDescendantIterator {
            holder,
            doc: Arc::clone(&node.doc),
            stack: vec![node.id],
            filter: DescFilter::parse(tag),
        })
    }

    /// The parent element proxy, or None at the tree root -- etree
    /// `getparent()`, fully native (one lock for the parent lookup, then the
    /// proxy cache).
    fn getparent(slf: &Bound<'_, Self>) -> PyResult<Option<Py<PyAny>>> {
        let py = slf.py();
        let (holder, parent_id) = {
            let cell = slf.borrow();
            let node = cell.node.bind(py).borrow();
            let guard = node
                .doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let pid = match guard.doc.parent(node.id) {
                // The document node is not an element; its children are roots.
                Some(p) if !matches!(guard.doc.node_kind(p), Some(NodeKind::Document)) => Some(p),
                _ => None,
            };
            drop(guard);
            (cell.holder.clone_ref(py), pid)
        };
        match parent_id {
            Some(pid) => {
                let holder = holder.bind(py).cast::<DocHolderBase>()?.clone();
                Ok(Some(DocHolderBase::proxy_for_id(
                    &holder,
                    pid.index(),
                    None,
                )?))
            }
            None => Ok(None),
        }
    }

    /// Native backing of `_Element.__iter__`: a lazy content-child iterator
    /// yielding identity-stable proxies one sibling hop at a time, so
    /// early-termination patterns (`next(iter(el))`) never pay for the whole
    /// child list. Callers that want the materialised list (indexing, full
    /// slices) use `_children_proxies` below instead.
    fn _iter_children(&self, py: Python<'_>) -> PyResult<ProxyChildIterator> {
        let holder: Py<DocHolderBase> = self
            .holder
            .bind(py)
            .cast::<DocHolderBase>()?
            .clone()
            .unbind();
        let node = self.node.bind(py).borrow();
        let first = {
            let guard = node
                .doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            guard.doc.first_child(node.id)
        };
        Ok(ProxyChildIterator {
            holder,
            doc: Arc::clone(&node.doc),
            next: first,
        })
    }

    /// The element's content children (elements, comments, PIs -- the ones
    /// lxml exposes as indexable children) as a list of cached proxies; backs
    /// `_Element.__getitem__` for full slices. Collects the ids under one
    /// lock, then materialises proxies through the cache.
    fn _children_proxies(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let (holder, ids) = {
            let cell = slf.borrow();
            let node = cell.node.bind(py).borrow();
            let guard = node
                .doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let mut ids = Vec::new();
            let mut child = guard.doc.first_child(node.id);
            while let Some(cid) = child {
                match guard.doc.node_kind(cid) {
                    Some(NodeKind::Element(_))
                    | Some(NodeKind::Comment(_))
                    | Some(NodeKind::ProcessingInstruction(_)) => ids.push(cid.index()),
                    _ => {}
                }
                child = guard.doc.next_sibling(cid);
            }
            drop(guard);
            (cell.holder.clone_ref(py), ids)
        };
        let holder = holder.bind(py).cast::<DocHolderBase>()?.clone();
        ids.into_iter()
            .map(|nid| DocHolderBase::proxy_for_id(&holder, nid, None))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// DocHolderBase -- native per-document identity proxy cache
// ---------------------------------------------------------------------------

/// Subclassable native base for `pyuppsala.etree._DocHolder`.
///
/// Owns one native `Document` plus the identity-stable proxy cache mapping
/// `node_id -> weakref(_Element)`, so repeated lookups of the same underlying
/// node return the *same* Python wrapper (`root[0] is root[0]`, matching
/// lxml). Runs the entire hit path (dict lookup + weakref upgrade) and the
/// miss path (construct `_Element` via the registered type object, insert a
/// callback-free weakref, opportunistic dead-entry sweep) in Rust; the pure
/// Python version of this cache was ~0.5 s of the pyFF full-sign profile and
/// its per-append dead sweep was quadratic during aggregation.
///
/// The Python `_DocHolder` subclass keeps only the cold, Python-flavoured
/// state (`_ns_counter`, `base_url`, `new_prefix`).
#[pyclass(subclass, weakref, name = "_DocHolderBase")]
struct DocHolderBase {
    /// The owned native document (a `Document` pyclass instance).
    doc: Py<Document>,
    /// node_id -> callback-free weakref to the live `_Element` wrapper.
    /// Callback-free deliberately: pyFF-style walks create a proxy per node
    /// and drop it immediately, so a death callback per proxy would dominate;
    /// dead entries are reclaimed by the bounded sweep below instead.
    proxies: std::collections::HashMap<usize, Py<pyo3::types::PyWeakrefReference>>,
    /// Cache size at which the next `proxy()` sweeps dead weakrefs; re-armed
    /// after each sweep to `max(256, 2 * live)` so transient walks hold a
    /// bounded number of tombstones and total sweep work stays O(nodes).
    sweep_at: usize,
    /// Interned Clark-notation tag strings, keyed by a hash of
    /// `(namespace_uri, local_name)` with a bucket vec for collisions
    /// (verified by piecewise comparison -- zero allocation on a hit). Every
    /// element after the first with the same qualified name returns the same
    /// `Py<PyString>`, so `.tag` reads stop allocating and equal tags become
    /// pointer-identical (hitting CPython's str identity fast path in `==`).
    /// A SAML tree has a few dozen unique QNames across tens of thousands of
    /// nodes, so the table stays tiny; renames simply hash to a different
    /// key, so there is nothing to invalidate.
    tag_table: std::collections::HashMap<u64, Vec<Py<pyo3::types::PyString>>>,
}

/// True if `s` is exactly the Clark notation `{ns}local` (or bare `local`
/// when `ns` is None) -- the zero-allocation verify for tag-table hits.
fn clark_eq(s: &str, ns: Option<&str>, local: &str) -> bool {
    match ns {
        Some(ns) => {
            s.len() == ns.len() + local.len() + 2
                && s.as_bytes()[0] == b'{'
                && &s[1..1 + ns.len()] == ns
                && s.as_bytes()[1 + ns.len()] == b'}'
                && &s[2 + ns.len()..] == local
        }
        None => s == local,
    }
}

/// Hash key for the tag table: mixes `(namespace_uri, local_name)`.
fn tag_key(ns: Option<&str>, local: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ns.hash(&mut h);
    local.hash(&mut h);
    h.finish()
}

impl DocHolderBase {
    /// Look up the interned Clark tag for `(namespace_uri, local_name)`.
    ///
    /// Read-only companion to [`DocHolderBase::intern_tag`]: a hit costs a
    /// hash lookup, a piecewise compare, and an incref -- no Python-object
    /// allocation, no Python code -- so it is safe to call while the document
    /// mutex (or a holder borrow) is held.
    fn lookup_tag(
        &self,
        py: Python<'_>,
        ns: Option<&str>,
        local: &str,
    ) -> PyResult<Option<Py<pyo3::types::PyString>>> {
        if let Some(bucket) = self.tag_table.get(&tag_key(ns, local)) {
            for s in bucket.iter() {
                // to_str is fine here: interned tags are always valid UTF-8 we
                // created ourselves, and reading a PyString runs no Python code.
                if clark_eq(s.bind(py).to_str()?, ns, local) {
                    return Ok(Some(s.clone_ref(py)));
                }
            }
        }
        Ok(None)
    }

    /// Return the interned `Py<PyString>` for the Clark tag of
    /// `(namespace_uri, local_name)`, formatting and caching it on first
    /// sight. Hits allocate nothing and return the same object every time.
    ///
    /// Takes the holder as a `Bound` rather than `&mut self` so no `RefCell`
    /// borrow is held while the Python string is allocated: `PyString::new`
    /// can trigger GC, and a finalizer may re-enter this holder (the same
    /// hazard `proxy_for_id` guards against). Callers must not hold the
    /// document mutex either -- use [`DocHolderBase::lookup_tag`] under the
    /// lock and call this only after releasing it.
    fn intern_tag(
        slf: &Bound<'_, DocHolderBase>,
        py: Python<'_>,
        ns: Option<&str>,
        local: &str,
    ) -> PyResult<Py<pyo3::types::PyString>> {
        if let Some(s) = slf.borrow().lookup_tag(py, ns, local)? {
            return Ok(s);
        }
        // Miss: allocate with no borrow held (see doc comment above).
        let clark = match ns {
            Some(ns) => format!("{{{}}}{}", ns, local),
            None => local.to_string(),
        };
        let obj = pyo3::types::PyString::new(py, &clark).unbind();
        // Re-check under the write borrow: a GC finalizer may have interned
        // the same tag while we allocated. Returning the existing entry keeps
        // identity stable (exactly one object per unique tag).
        let mut cell = slf.borrow_mut();
        let bucket = cell.tag_table.entry(tag_key(ns, local)).or_default();
        for s in bucket.iter() {
            if clark_eq(s.bind(py).to_str()?, ns, local) {
                return Ok(s.clone_ref(py));
            }
        }
        bucket.push(obj.clone_ref(py));
        Ok(obj)
    }

    /// Return the identity-stable `_Element` for node id `nid`, creating and
    /// caching it if no live wrapper exists. `node` supplies an existing
    /// native `Node` handle to reuse on the miss path (avoids re-creating
    /// one); pass `None` to build the handle only when actually needed.
    ///
    /// Structured in three phases so the `RefCell` borrow is never held
    /// across a call into Python: constructing `_Element` can trigger GC,
    /// and a finalizer may re-enter `proxy` on this same holder, which would
    /// panic on an overlapping borrow.
    fn proxy_for_id(
        slf: &Bound<'_, DocHolderBase>,
        nid: usize,
        node: Option<Py<Node>>,
    ) -> PyResult<Py<PyAny>> {
        use pyo3::types::PyWeakrefMethods;
        let py = slf.py();
        // Phase 1: cache hit under a short borrow. Upgrading a weakref reads
        // a pointer and increfs -- it runs no Python code, so holding the
        // borrow here is safe. On a hit the cached wrapper is returned as-is:
        // its stored `Node` is just `(Arc, node_id)`, both invariant for the
        // life of the document, so no refresh is needed.
        {
            let cell = slf.borrow();
            if let Some(wref) = cell.proxies.get(&nid) {
                if let Some(el) = wref.bind(py).upgrade() {
                    return Ok(el.unbind());
                }
                // Dead tombstone: fall through and recreate (the insert below
                // overwrites it).
            }
        }
        // Phase 2: miss -- construct outside any borrow.
        let node_obj = match node {
            Some(n) => n,
            None => {
                let shared = {
                    let cell = slf.borrow();
                    let doc = cell.doc.bind(py).borrow();
                    Arc::clone(&doc.inner)
                };
                Py::new(
                    py,
                    Node {
                        doc: shared,
                        id: NodeId::new(nid),
                    },
                )?
            }
        };
        let eltype = ELEMENT_TYPE
            .get(py)
            .ok_or_else(|| PyRuntimeError::new_err("etree element type not registered"))?;
        // `type.__call__` -> PyO3 `tp_new` -> `ElementBase::new`; `_Element`
        // defines no `__init__`, so no Python frame runs here.
        let el = eltype.bind(py).call1((slf, node_obj, nid))?;
        let wref = pyo3::types::PyWeakrefReference::new(&el)?;
        // Phase 3: insert + opportunistic sweep under a fresh borrow.
        {
            let mut cell = slf.borrow_mut();
            cell.proxies.insert(nid, wref.unbind());
            if cell.proxies.len() >= cell.sweep_at {
                cell.proxies.retain(|_, r| r.bind(py).upgrade().is_some());
                cell.sweep_at = std::cmp::max(256, cell.proxies.len() * 2);
            }
        }
        Ok(el.unbind())
    }
}

#[pymethods]
impl DocHolderBase {
    #[new]
    fn new(doc: Py<Document>) -> Self {
        DocHolderBase {
            doc,
            proxies: std::collections::HashMap::new(),
            sweep_at: 256,
            tag_table: std::collections::HashMap::new(),
        }
    }

    /// The owned native `Document`.
    #[getter]
    fn doc(&self, py: Python<'_>) -> Py<Document> {
        self.doc.clone_ref(py)
    }

    /// Return the identity-stable `_Element` wrapper for `node` (or None for
    /// None), creating and caching it on first access. See the class docs;
    /// this is the native port of the former Python `_DocHolder.proxy`.
    fn proxy(slf: &Bound<'_, Self>, node: Option<Py<Node>>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let Some(node) = node else {
            return Ok(py.None());
        };
        let nid = node.bind(py).borrow().id.index();
        DocHolderBase::proxy_for_id(slf, nid, Some(node))
    }

    /// Move any live proxies from the source subtree rooted at `snode` (in
    /// this holder's document) onto the cloned subtree rooted at `dnode` (in
    /// `dst`'s document), preserving Python identity across cross-document
    /// moves -- the native port of the former `_repoint_subtree`.
    ///
    /// Runs in two phases: (A) collect the `(source id, clone id)` pairs by a
    /// lock-step arena walk under both document locks (pure reads, no Python
    /// allocation while locked, so GC cannot re-enter and try to re-lock);
    /// (B) with the GIL only, re-point each live wrapper and move its cache
    /// entry. The upfront dead sweep keeps a holder whose proxies are all
    /// dead on the O(1) fast path instead of degrading the walk to
    /// O(proxies * subtree) (the historical quadratic-append bug).
    fn repoint_subtree(
        slf: &Bound<'_, Self>,
        dst: &Bound<'_, DocHolderBase>,
        snode: PyRef<'_, Node>,
        dnode: PyRef<'_, Node>,
    ) -> PyResult<()> {
        use pyo3::types::PyWeakrefMethods;
        let py = slf.py();
        if slf.is(dst) {
            // Same holder on both sides: nothing to move between caches.
            return Ok(());
        }
        // Upfront dead sweep + emptiness fast path.
        {
            let mut cell = slf.borrow_mut();
            cell.proxies.retain(|_, r| r.bind(py).upgrade().is_some());
            cell.sweep_at = std::cmp::max(256, cell.proxies.len() * 2);
            if cell.proxies.is_empty() {
                return Ok(());
            }
        }
        // Phase A: lock-step pair collection. Both mutexes are taken in the
        // same fixed global order (by Arc address) as `Document::import_subtree`
        // so the two-document lock discipline stays unique crate-wide.
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        {
            let src_shared = Arc::clone(&snode.doc);
            let dst_shared = Arc::clone(&dnode.doc);
            let lock_err = |e: std::sync::PoisonError<std::sync::MutexGuard<'_, DocWithInput>>| {
                PyRuntimeError::new_err(e.to_string())
            };
            let (src_guard, dst_guard) = if Arc::ptr_eq(&src_shared, &dst_shared) {
                // Clones within one document share a single lock.
                (src_shared.lock().map_err(lock_err)?, None)
            } else if Arc::as_ptr(&dst_shared) < Arc::as_ptr(&src_shared) {
                let d = dst_shared.lock().map_err(lock_err)?;
                let s = src_shared.lock().map_err(lock_err)?;
                (s, Some(d))
            } else {
                let s = src_shared.lock().map_err(lock_err)?;
                let d = dst_shared.lock().map_err(lock_err)?;
                (s, Some(d))
            };
            let sdoc = &src_guard.doc;
            let ddoc = match &dst_guard {
                Some(g) => &g.doc,
                None => sdoc,
            };
            // Children are cloned in document order, so a positional lock-step
            // walk pairs source and clone nodes exactly (same invariant the
            // Python `zip(snode.children, dnode.children)` walk relied on).
            let mut stack = vec![(snode.id, dnode.id)];
            while let Some((s, d)) = stack.pop() {
                pairs.push((s.index(), d.index()));
                let mut sc = sdoc.first_child(s);
                let mut dc = ddoc.first_child(d);
                while let (Some(scn), Some(dcn)) = (sc, dc) {
                    stack.push((scn, dcn));
                    sc = sdoc.next_sibling(scn);
                    dc = ddoc.next_sibling(dcn);
                }
            }
        }
        // Phase B: cache surgery, GIL only (no document locks held).
        let dst_shared = Arc::clone(&dnode.doc);
        let dst_obj: Py<PyAny> = dst.clone().unbind().into_any();
        for (sid, did) in pairs {
            let wref = slf.borrow_mut().proxies.remove(&sid);
            let Some(wref) = wref else { continue };
            let Some(el) = wref.bind(py).upgrade() else {
                continue; // dead tombstone: reclaimed by the remove above
            };
            let el_base = el.cast::<ElementBase>()?;
            let new_node = Py::new(
                py,
                Node {
                    doc: Arc::clone(&dst_shared),
                    id: NodeId::new(did),
                },
            )?;
            {
                let mut b = el_base.borrow_mut();
                b.holder = dst_obj.clone_ref(py);
                b.node = new_node;
                b.node_id = did;
            }
            // The weakref still points at `el`; reuse it under the new key.
            dst.borrow_mut().proxies.insert(did, wref);
            if slf.borrow().proxies.is_empty() {
                // Every live source proxy has been moved; skip the rest of
                // the (possibly large) subtree.
                break;
            }
        }
        Ok(())
    }

    /// Number of cache entries (live + not-yet-swept tombstones); for tests.
    fn _proxy_cache_len(&self) -> usize {
        self.proxies.len()
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
// DescendantIterator
// ---------------------------------------------------------------------------

/// The tag filter applied while walking a subtree (parsed once when the
/// iterator is created so the hot `__next__` loop does no string work).
enum DescFilter {
    /// `tag=None`: elements, comments and processing instructions.
    All,
    /// `tag="*"`: elements only.
    Elements,
    /// A specific element name. `ns` is `None` for the no-namespace case (a
    /// bare local name or an empty `{}` namespace), matching lxml.
    Named { ns: Option<String>, local: String },
}

impl DescFilter {
    /// Parse an lxml-style tag argument into a filter. `None` -> `All`,
    /// `"*"` -> `Elements`, `"{ns}local"`/`"local"` -> `Named`.
    fn parse(tag: Option<&str>) -> DescFilter {
        match tag {
            None => DescFilter::All,
            Some("*") => DescFilter::Elements,
            Some(t) => {
                if let Some(rest) = t.strip_prefix('{') {
                    if let Some(idx) = rest.find('}') {
                        let ns = &rest[..idx];
                        let local = &rest[idx + 1..];
                        return DescFilter::Named {
                            // An empty namespace ("{}local") is the no-namespace
                            // case in lxml, so normalise "" to None.
                            ns: if ns.is_empty() {
                                None
                            } else {
                                Some(ns.to_string())
                            },
                            local: local.to_string(),
                        };
                    }
                }
                DescFilter::Named {
                    ns: None,
                    local: t.to_string(),
                }
            }
        }
    }
}

/// A lazy, native pre-order descendant iterator (see `Node::iter_descendants`).
///
/// Holds an explicit stack of node ids. Each `__next__` acquires the document
/// lock once, then advances through the tree (pushing children, skipping
/// non-matching nodes) until it finds the next match or the stack empties.
/// Children are pushed in reverse so they pop in document order, giving the
/// pre-order (parent before children) sequence lxml produces.
#[pyclass]
struct DescendantIterator {
    doc: SharedDoc,
    stack: Vec<NodeId>,
    filter: DescFilter,
}

/// Advance a pre-order descendant walk to the next node matching `filter`,
/// returning its id, or `None` when the stack is exhausted. Shared by
/// `DescendantIterator` (yields `Node`s) and `ProxyDescendantIterator`
/// (yields cached `_Element` proxies). Children are pushed in reverse so
/// they pop in document order, giving the pre-order sequence lxml produces.
fn advance_desc_walk(
    doc: &UDocument<'static>,
    stack: &mut Vec<NodeId>,
    filter: &DescFilter,
) -> Option<NodeId> {
    while let Some(id) = stack.pop() {
        // Push this node's children in reverse document order so the first
        // child is popped next (pre-order). Done before the match check so
        // we descend into matching nodes too.
        let start = stack.len();
        let mut child = doc.first_child(id);
        while let Some(cid) = child {
            stack.push(cid);
            child = doc.next_sibling(cid);
        }
        stack[start..].reverse();

        let matched = match filter {
            DescFilter::All => matches!(
                doc.node_kind(id),
                Some(NodeKind::Element(_))
                    | Some(NodeKind::Comment(_))
                    | Some(NodeKind::ProcessingInstruction(_))
            ),
            DescFilter::Elements => {
                matches!(doc.node_kind(id), Some(NodeKind::Element(_)))
            }
            DescFilter::Named { ns, local } => {
                matches!(doc.element(id), Some(e) if e.name.matches(ns.as_deref(), local))
            }
        };
        if matched {
            return Some(id);
        }
    }
    None
}

#[pymethods]
impl DescendantIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> PyResult<Option<Node>> {
        let guard = self
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(
            advance_desc_walk(&guard.doc, &mut self.stack, &self.filter).map(|id| Node {
                doc: Arc::clone(&self.doc),
                id,
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// ProxyDescendantIterator
// ---------------------------------------------------------------------------

/// A lazy pre-order descendant iterator that yields identity-stable etree
/// `_Element` proxies directly (the backing of `_Element.iter`).
///
/// Same native walk as `DescendantIterator`, but each `__next__` finishes in
/// the holder's proxy cache instead of materialising a throwaway `Node`
/// pyobject that Python then re-wraps: a cache hit allocates nothing at all,
/// and a miss builds the `Node` + `_Element` entirely in Rust. This removes
/// the per-match Python generator frame + `proxy()` call that made
/// `etree.iter` ~0.5 s of the pyFF full-sign profile.
#[pyclass]
struct ProxyDescendantIterator {
    holder: Py<DocHolderBase>,
    doc: SharedDoc,
    stack: Vec<NodeId>,
    filter: DescFilter,
}

#[pymethods]
impl ProxyDescendantIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(slf: &Bound<'_, Self>) -> PyResult<Option<Py<PyAny>>> {
        let py = slf.py();
        // Find the next matching node id under the document lock, releasing
        // both the lock and the iterator borrow before touching the proxy
        // cache (proxy construction may run Python code via `tp_new`).
        let (holder, next_id) = {
            let mut cell = slf.borrow_mut();
            // Destructure once so the lock guard, the stack and the filter
            // borrow disjoint fields of the same `PyRefMut`.
            let ProxyDescendantIterator {
                holder,
                doc,
                stack,
                filter,
            } = &mut *cell;
            let guard = doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let id = advance_desc_walk(&guard.doc, stack, filter);
            drop(guard);
            (holder.clone_ref(py), id)
        };
        match next_id {
            Some(id) => {
                let el = DocHolderBase::proxy_for_id(holder.bind(py), id.index(), None)?;
                Ok(Some(el))
            }
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// ProxyChildIterator
// ---------------------------------------------------------------------------

/// A lazy content-child iterator that yields identity-stable etree
/// `_Element` proxies (the backing of `_Element.__iter__`).
///
/// Walks the sibling chain one hop per `__next__` instead of materialising
/// every child's proxy up front, so early-termination patterns
/// (`next(iter(el))`, `zip`, `any(...)`) only pay for the children they
/// actually consume -- with the eager list, the first item of a wide element
/// cost O(children). Full iteration stays native: each step is one sibling
/// hop under the document lock plus a proxy-cache lookup, with no Python
/// frame in between.
///
/// Like lxml, the walk follows the *live* sibling chain: restructuring the
/// children mid-iteration redirects the remaining walk accordingly.
#[pyclass]
struct ProxyChildIterator {
    holder: Py<DocHolderBase>,
    doc: SharedDoc,
    /// The next sibling to consider (not yet filtered to content kinds);
    /// `None` when the chain is exhausted.
    next: Option<NodeId>,
}

#[pymethods]
impl ProxyChildIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(slf: &Bound<'_, Self>) -> PyResult<Option<Py<PyAny>>> {
        let py = slf.py();
        // Advance to the next content child (element/comment/PI -- the kinds
        // lxml exposes as children; text/CDATA surface via .text/.tail) under
        // the document lock, releasing both the lock and the iterator borrow
        // before touching the proxy cache (proxy construction may run Python
        // code via `tp_new`).
        let (holder, found) = {
            let mut cell = slf.borrow_mut();
            let ProxyChildIterator { holder, doc, next } = &mut *cell;
            let guard = doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let mut cur = *next;
            let mut found = None;
            while let Some(cid) = cur {
                cur = guard.doc.next_sibling(cid);
                match guard.doc.node_kind(cid) {
                    Some(NodeKind::Element(_))
                    | Some(NodeKind::Comment(_))
                    | Some(NodeKind::ProcessingInstruction(_)) => {
                        found = Some(cid);
                        break;
                    }
                    _ => {}
                }
            }
            *next = cur;
            drop(guard);
            (holder.clone_ref(py), found)
        };
        match found {
            Some(id) => Ok(Some(DocHolderBase::proxy_for_id(
                holder.bind(py),
                id.index(),
                None,
            )?)),
            None => Ok(None),
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
    /// * ``forbid_dtd`` - when True, reject any ``<!DOCTYPE`` at parse time.
    /// * ``forbid_entities`` - when True, reject ``<!ENTITY>`` declarations
    ///   (general and parameter) while still allowing the rest of a DTD.
    ///
    /// .. warning::
    ///    Do not source the resource-limit kwargs (``max_depth``,
    ///    ``max_entity_expansion``) from untrusted input. An attacker who
    ///    controls those caps can re-enable the corresponding DoS attack class
    ///    (deep-nesting stack overflow, billion-laughs entity expansion). This
    ///    does not apply to ``forbid_dtd`` / ``forbid_entities``, which only
    ///    tighten parsing.
    #[new]
    #[pyo3(signature = (xml, *, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
    fn new(
        py: Python<'_>,
        xml: &str,
        max_depth: Option<u32>,
        max_entity_expansion: Option<usize>,
        namespace_aware: Option<bool>,
        forbid_dtd: Option<bool>,
        forbid_entities: Option<bool>,
    ) -> PyResult<Self> {
        // Copy the input to an owned String while attached to Python, so the
        // parse below can run detached (GIL released): only owned data may
        // cross into the detach closure.
        let input = xml.to_string();
        let parser = build_parser(
            max_depth,
            max_entity_expansion,
            namespace_aware,
            forbid_dtd,
            forbid_entities,
        );
        // Parsing is pure Rust over `input` and touches no Python objects, so
        // release the GIL for its duration: other Python threads keep running,
        // and N threads parsing concurrently genuinely use N cores. The PyErr
        // is only built after re-attaching.
        let parsed = py.detach(|| parser.parse(&input).map(|d| d.into_static()));
        let doc = parsed.map_err(xml_error_to_pyerr)?;
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
    #[pyo3(signature = (data, *, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
    fn from_bytes(
        py: Python<'_>,
        data: &[u8],
        max_depth: Option<u32>,
        max_entity_expansion: Option<usize>,
        namespace_aware: Option<bool>,
        forbid_dtd: Option<bool>,
        forbid_entities: Option<bool>,
    ) -> PyResult<Document> {
        // Decode while attached (borrows the Python buffer); parse detached.
        let input = decode_xml_bytes(data)?;
        let parser = build_parser(
            max_depth,
            max_entity_expansion,
            namespace_aware,
            forbid_dtd,
            forbid_entities,
        );
        let parsed = py.detach(|| parser.parse(&input).map(|d| d.into_static()));
        let doc = parsed.map_err(xml_error_to_pyerr)?;
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

    /// The raw ``<!DOCTYPE ...>`` declaration preserved from the source, or None.
    ///
    /// Uppsala preserves the document type declaration verbatim (including the
    /// ``<!DOCTYPE`` and trailing ``>``) for round-trip fidelity, but does not
    /// process it. Returns None for documents without a DOCTYPE or for
    /// programmatically constructed documents. Use
    /// ``to_xml_with_options(include_doctype=True)`` to serialize it back out.
    #[getter]
    fn doctype(&self) -> PyResult<Option<String>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(guard.doc.doctype.as_ref().map(|dt| dt.to_string()))
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
        validate_ncname(local_name, "element")?;
        let prefix = validate_qname_parts(namespace_uri, prefix)?;
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
        validate_pi_target(target)?;
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
    ///
    /// Both node handles must come from this ``Document`` because the native
    /// DOM interprets their ``NodeId`` values in the receiver document.
    fn append_child(&self, parent: &Node, child: &Node) -> PyResult<()> {
        ensure_node_in_document(&self.inner, parent, "parent")?;
        ensure_node_in_document(&self.inner, child, "child")?;
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.append_child(parent.id, child.id);
        Ok(())
    }

    /// Deep-copy ``source`` (a node from a *different* Document) and its whole
    /// subtree into this document, returning the new detached node.
    ///
    /// `NodeId`s are document-scoped, so cross-document `append`/`deepcopy` in the
    /// etree layer must clone rather than reparent. This does the entire subtree
    /// copy in one native pass (uppsala `Document::import_subtree`) instead of one
    /// FFI call per node, which was the dominant cost of pyFF's aggregation step.
    /// The element's own namespace declarations are copied; namespaces inherited
    /// from ancestors outside the subtree remain the caller's responsibility.
    ///
    /// Locks both documents, in a fixed global order (by `Arc` address) so
    /// concurrent imports in opposite directions cannot deadlock. The source
    /// must be a different `Document`; importing from the same document raises
    /// ValueError (use the move/detach path instead).
    fn import_subtree(&self, source: &Node) -> PyResult<Node> {
        if Arc::ptr_eq(&self.inner, &source.doc) {
            return Err(PyValueError::new_err(
                "import_subtree requires a node from a different Document",
            ));
        }
        // Acquire both document mutexes in a fixed global order (by Arc address)
        // so two threads importing in opposite directions (A<-B and B<-A) cannot
        // deadlock. Only the lock order differs between branches; the tuple is
        // always (source guard, dest guard).
        let (src_guard, mut dst_guard) = if Arc::as_ptr(&self.inner) < Arc::as_ptr(&source.doc) {
            let dst_guard = self
                .inner
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let src_guard = source
                .doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            (src_guard, dst_guard)
        } else {
            let src_guard = source
                .doc
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let dst_guard = self
                .inner
                .lock()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            (src_guard, dst_guard)
        };
        let new_id = dst_guard
            .doc
            .import_subtree(&src_guard.doc, source.id)
            .ok_or_else(|| {
                PyValueError::new_err("cannot import this node (document root or attribute node)")
            })?;
        Ok(Node {
            doc: Arc::clone(&self.inner),
            id: new_id,
        })
    }

    /// Add or replace an `xmlns` declaration on an element node.
    ///
    /// `prefix=None` sets the default namespace (`xmlns="uri"`); otherwise sets
    /// `xmlns:prefix="uri"`. Used by the etree layer so namespaced trees built
    /// in memory serialize with correct namespace declarations. Raises
    /// ValueError if `node` is not an element, or if the declaration is one the
    /// XML Namespaces spec reserves (the `xmlns` prefix, rebinding `xml`/the XML
    /// namespace, or declaring the `xmlns` namespace). The node handle must
    /// come from this ``Document`` because its ``NodeId`` is document-scoped.
    #[pyo3(signature = (node, prefix, uri))]
    fn set_namespace_declaration(
        &self,
        node: &Node,
        prefix: Option<&str>,
        uri: &str,
    ) -> PyResult<()> {
        ensure_node_in_document(&self.inner, node, "node")?;
        let prefix = validate_prefix(prefix)?;
        validate_ns_declaration(prefix, uri)?;
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
    ///
    /// All three node handles must come from this ``Document`` because their
    /// ``NodeId`` values are document-scoped.
    fn insert_before(&self, parent: &Node, new_child: &Node, reference: &Node) -> PyResult<()> {
        ensure_node_in_document(&self.inner, parent, "parent")?;
        ensure_node_in_document(&self.inner, new_child, "new_child")?;
        ensure_node_in_document(&self.inner, reference, "reference")?;
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
    ///
    /// All three node handles must come from this ``Document`` because their
    /// ``NodeId`` values are document-scoped.
    fn insert_after(&self, parent: &Node, new_child: &Node, reference: &Node) -> PyResult<()> {
        ensure_node_in_document(&self.inner, parent, "parent")?;
        ensure_node_in_document(&self.inner, new_child, "new_child")?;
        ensure_node_in_document(&self.inner, reference, "reference")?;
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
    ///
    /// Both node handles must come from this ``Document`` because the native
    /// DOM interprets their ``NodeId`` values in the receiver document.
    fn remove_child(&self, parent: &Node, child: &Node) -> PyResult<()> {
        ensure_node_in_document(&self.inner, parent, "parent")?;
        ensure_node_in_document(&self.inner, child, "child")?;
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.remove_child(parent.id, child.id);
        Ok(())
    }

    /// Replace old_child with new_child under the given parent.
    ///
    /// All three node handles must come from this ``Document``. The native
    /// DOM uses document-scoped ``NodeId`` values, so accepting a foreign
    /// handle here would reinterpret that id inside the receiver document.
    fn replace_child(&self, parent: &Node, new_child: &Node, old_child: &Node) -> PyResult<()> {
        ensure_node_in_document(&self.inner, parent, "parent")?;
        ensure_node_in_document(&self.inner, new_child, "new_child")?;
        ensure_node_in_document(&self.inner, old_child, "old_child")?;
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
    /// append_child, insert_before, or insert_after. The node handle must come
    /// from this ``Document`` because its ``NodeId`` is document-scoped.
    fn detach(&self, node: &Node) -> PyResult<()> {
        ensure_node_in_document(&self.inner, node, "node")?;
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        guard.doc.detach(node.id);
        Ok(())
    }

    // -- Serialization -------------------------------------------------------

    /// Serialize the document to a compact XML string.
    fn to_xml(&self, py: Python<'_>) -> PyResult<String> {
        // Whole-document serialization is pure Rust producing a String, so it
        // runs detached; the doc mutex is taken and released inside the
        // closure (lock order: GIL -> doc mutex, never the reverse).
        let shared = Arc::clone(&self.inner);
        py.detach(|| {
            let guard = shared.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(guard.doc.to_xml())
        })
        .map_err(PyRuntimeError::new_err)
    }

    /// Serialize the document to an XML string with formatting options.
    ///
    /// Args:
    ///     indent: Indentation string (e.g. "  " for 2-space indent), or None for compact.
    ///     expand_empty_elements: If True, write <foo></foo> instead of <foo/>.
    ///     include_doctype: If True, serialize the preserved ``<!DOCTYPE ...>``
    ///         declaration (if the document had one) ahead of the root element.
    ///         Defaults to False so a parsed DTD is not re-emitted unless the
    ///         caller deliberately opts into round-tripping it.
    #[pyo3(signature = (indent=None, expand_empty_elements=false, include_doctype=false))]
    fn to_xml_with_options(
        &self,
        py: Python<'_>,
        indent: Option<&str>,
        expand_empty_elements: bool,
        include_doctype: bool,
    ) -> PyResult<String> {
        let opts = make_write_options(indent, expand_empty_elements, include_doctype);
        let shared = Arc::clone(&self.inner);
        py.detach(|| {
            let guard = shared.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(guard.doc.to_xml_with_options(&opts))
        })
        .map_err(PyRuntimeError::new_err)
    }

    /// Write the document to a file.
    fn write_to_file(&self, py: Python<'_>, path: &str) -> PyResult<()> {
        // Serialization + file I/O both benefit from a detached GIL.
        let shared = Arc::clone(&self.inner);
        let path = path.to_string();
        py.detach(|| {
            let guard = shared.lock().map_err(|e| e.to_string())?;
            let mut file =
                std::fs::File::create(&path).map_err(|e| format!("Cannot create file: {}", e))?;
            guard
                .doc
                .write_to(&mut file)
                .map_err(|e| format!("Write error: {}", e))
        })
        .map_err(PyRuntimeError::new_err)
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

    fn __str__(&self, py: Python<'_>) -> PyResult<String> {
        self.to_xml(py)
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
    /// ``max_node_visits`` overrides the default per-evaluation node-visit
    /// budget (default ``DEFAULT_MAX_XPATH_NODE_VISITS``, 100_000) that bounds
    /// how many nodes a single expression may traverse, guarding against
    /// algorithmic-complexity denial-of-service on adversarial documents.
    ///
    /// .. warning::
    ///    Do not source ``max_depth`` or ``max_node_visits`` from untrusted
    ///    input - an attacker who controls these caps can re-enable XPath
    ///    stack-overflow or node-traversal denial-of-service attacks.
    #[new]
    #[pyo3(signature = (*, max_depth=None, max_node_visits=None))]
    fn new(max_depth: Option<u32>, max_node_visits: Option<usize>) -> Self {
        let mut inner = UXPathEvaluator::new();
        if let Some(d) = max_depth {
            inner = inner.with_max_depth(d);
        }
        if let Some(v) = max_node_visits {
            inner = inner.with_max_node_visits(v);
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
    /// depending on the XPath result type. If a context node is supplied, it
    /// must belong to ``doc`` because XPath context ``NodeId`` values are
    /// document-scoped.
    #[pyo3(signature = (doc, expr, context=None))]
    fn evaluate<'py>(
        &self,
        py: Python<'py>,
        doc: &Document,
        expr: &str,
        context: Option<&Node>,
    ) -> PyResult<Py<PyAny>> {
        let context_id = match context {
            Some(n) => {
                ensure_node_in_document(&doc.inner, n, "context")?;
                Some(n.id)
            }
            None => None,
        };
        let inner_doc = doc
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let ctx_id = context_id.unwrap_or_else(|| inner_doc.doc.root());
        let result = self
            .inner
            .evaluate(&inner_doc.doc, ctx_id, expr)
            .map_err(xml_error_to_pyerr)?;
        drop(inner_doc); // release lock before building Python objects
        xpath_value_to_py(py, &doc.inner, result)
    }

    /// Evaluate an XPath expression and return matching nodes.
    ///
    /// If a context node is supplied, it must belong to ``doc`` because XPath
    /// context ``NodeId`` values are document-scoped.
    #[pyo3(signature = (doc, expr, context=None))]
    fn select(&self, doc: &Document, expr: &str, context: Option<&Node>) -> PyResult<Vec<Node>> {
        let context_id = match context {
            Some(n) => {
                ensure_node_in_document(&doc.inner, n, "context")?;
                Some(n.id)
            }
            None => None,
        };
        let inner_doc = doc
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let ctx_id = context_id.unwrap_or_else(|| inner_doc.doc.root());
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
    fn new(py: Python<'_>, schema_xml: &str) -> PyResult<Self> {
        // Schema parse + compile are pure Rust; run them with the GIL
        // released (owned copy of the input crosses into the closure).
        let xml = schema_xml.to_string();
        let built = py.detach(|| {
            let schema_doc = uppsala::parse(&xml)?.into_static();
            UXsdValidator::from_schema(&schema_doc)
        });
        Ok(XsdValidator {
            inner: built.map_err(xml_error_to_pyerr)?,
        })
    }

    /// Create a validator from an XSD schema string, resolving external
    /// includes/imports relative to the given base path.
    #[staticmethod]
    fn from_file(py: Python<'_>, schema_xml: &str, base_path: &str) -> PyResult<XsdValidator> {
        let xml = schema_xml.to_string();
        let base = std::path::PathBuf::from(base_path);
        let built = py.detach(|| {
            let schema_doc = uppsala::parse(&xml)?.into_static();
            UXsdValidator::from_schema_with_base_path(&schema_doc, Some(base.as_path()))
        });
        Ok(XsdValidator {
            inner: built.map_err(xml_error_to_pyerr)?,
        })
    }

    /// Configure whether QName/NOTATION length facets are enforced.
    fn set_enforce_qname_length_facets(&mut self, enforce: bool) {
        self.inner.set_enforce_qname_length_facets(enforce);
    }

    /// Configure lenient validation of built-in datatypes.
    ///
    /// Off by default (strict). When enabled, a handful of built-in datatype
    /// checks that are stricter than libxml2 are relaxed to match it -- notably
    /// ``anyURI`` values containing a space are accepted (libxml2/lxml also
    /// accept them). Turn this on for lxml-compatible validation of real-world
    /// documents (e.g. SAML metadata whose ``anyURI`` values contain spaces).
    fn set_lenient(&mut self, lenient: bool) {
        self.inner.set_lenient(lenient);
    }

    /// Validate an XML document against this schema.
    ///
    /// Returns a list of ValidationError objects. An empty list means valid.
    fn validate(&self, py: Python<'_>, doc: &Document) -> PyResult<Vec<ValidationErrorPy>> {
        // Validation walks the whole document in pure Rust: release the GIL
        // and take the document lock inside the detached closure (lock order
        // is always GIL -> doc mutex; the closure releases the mutex before
        // re-attaching, so a GIL-holding thread waiting on it cannot deadlock).
        let shared = Arc::clone(&doc.inner);
        let validator = &self.inner;
        let errors = py
            .detach(|| {
                let inner_doc = shared.lock().map_err(|e| e.to_string())?;
                Ok::<_, String>(validator.validate(&inner_doc.doc))
            })
            .map_err(PyRuntimeError::new_err)?;
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
    fn validate_str(&self, py: Python<'_>, xml: &str) -> PyResult<Vec<ValidationErrorPy>> {
        // Parse + validate detached (see `validate`); owned input only.
        let input = xml.to_string();
        let validator = &self.inner;
        let errors = py
            .detach(|| {
                let doc = uppsala::parse(&input)?.into_static();
                Ok::<_, uppsala::XmlError>(validator.validate(&doc))
            })
            .map_err(xml_error_to_pyerr)?;
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
    fn is_valid(&self, py: Python<'_>, doc: &Document) -> PyResult<bool> {
        let shared = Arc::clone(&doc.inner);
        let validator = &self.inner;
        py.detach(|| {
            let inner_doc = shared.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(validator.validate(&inner_doc.doc).is_empty())
        })
        .map_err(PyRuntimeError::new_err)
    }

    /// Check if an XML string is valid. Returns True/False.
    fn is_valid_str(&self, py: Python<'_>, xml: &str) -> PyResult<bool> {
        let input = xml.to_string();
        let validator = &self.inner;
        Ok(py.detach(|| match uppsala::parse(&input) {
            Ok(d) => validator.validate(&d.into_static()).is_empty(),
            Err(_) => false,
        }))
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
    fn start_element(&mut self, name: &str, attrs: Option<Vec<(String, String)>>) -> PyResult<()> {
        validate_xml_name(name, "element")?;
        let attr_refs = writer_attr_refs(&attrs)?;
        self.inner.start_element(name, &attr_refs);
        Ok(())
    }

    /// End the current element.
    fn end_element(&mut self, name: &str) -> PyResult<()> {
        validate_xml_name(name, "element")?;
        self.inner.end_element(name);
        Ok(())
    }

    /// Write a self-closing empty element: <name/>
    #[pyo3(signature = (name, attrs=None))]
    fn empty_element(&mut self, name: &str, attrs: Option<Vec<(String, String)>>) -> PyResult<()> {
        validate_xml_name(name, "element")?;
        let attr_refs = writer_attr_refs(&attrs)?;
        self.inner.empty_element(name, &attr_refs);
        Ok(())
    }

    /// Write an expanded empty element: <name></name>
    #[pyo3(signature = (name, attrs=None))]
    fn empty_element_expanded(
        &mut self,
        name: &str,
        attrs: Option<Vec<(String, String)>>,
    ) -> PyResult<()> {
        validate_xml_name(name, "element")?;
        let attr_refs = writer_attr_refs(&attrs)?;
        self.inner.empty_element_expanded(name, &attr_refs);
        Ok(())
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
    fn processing_instruction(&mut self, target: &str, data: Option<&str>) -> PyResult<()> {
        validate_pi_target(target)?;
        self.inner.processing_instruction(target, data);
        Ok(())
    }

    /// Write raw XML content (not escaped).
    fn raw(&mut self, xml: &str) {
        self.inner.raw(xml);
    }

    /// Return the accumulated XML as a string.
    // Exposed to Python as `to_string()`; the Rust name differs so it is not an
    // inherent `to_string` (which would shadow the `ToString`/`Display` idiom).
    #[pyo3(name = "to_string")]
    fn to_string_py(&self) -> String {
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

/// A compiled XSLT 1.0 stylesheet.
///
/// Compiling once and transforming many documents avoids re-parsing and
/// re-compiling the stylesheet on every call (the `pyuppsala.etree.XSLT`
/// facade caches one of these per stylesheet). The compiled form fully owns
/// its data, so the stylesheet text need not outlive this object.
#[pyclass(name = "Xslt")]
struct Xslt {
    inner: uppsala::xslt::Stylesheet,
}

#[pymethods]
impl Xslt {
    /// Compile an XSLT 1.0 stylesheet from its XML source text.
    ///
    /// ``exslt`` enables the opt-in EXSLT extension-function library
    /// (``str:``/``math:``/``set:``/``exsl:``); ``date:date-time()`` is always
    /// available. Defaults to ``True`` to match lxml, which ships EXSLT on.
    /// ``max_depth`` overrides the template-activation recursion cap.
    #[new]
    #[pyo3(signature = (stylesheet_xml, *, exslt=true, max_depth=None))]
    fn new(
        py: Python<'_>,
        stylesheet_xml: &str,
        exslt: bool,
        max_depth: Option<u32>,
    ) -> PyResult<Self> {
        // Owned copy while attached; stylesheet parse + compile are pure Rust,
        // so they run with the GIL released (see Document::new for the rule).
        let xml = stylesheet_xml.to_string();
        let compiled = py.detach(|| {
            let style_doc = UParser::new().parse(&xml)?;
            uppsala::xslt::Stylesheet::compile(&style_doc)
        });
        let mut sheet = compiled.map_err(xml_error_to_pyerr)?;
        if let Some(d) = max_depth {
            sheet = sheet.set_max_depth(d);
        }
        sheet = sheet.with_exslt(exslt);
        Ok(Xslt { inner: sheet })
    }

    /// Apply the stylesheet to a source XML string, returning the serialized
    /// result. The source is parsed and prepared for XPath internally.
    fn transform(&self, py: Python<'_>, source_xml: &str) -> PyResult<String> {
        // Parse + transform are pure Rust over owned data; release the GIL so
        // concurrent transforms (e.g. pyFF's per-entity tidy.xsl) parallelize.
        let xml = source_xml.to_string();
        let sheet = &self.inner;
        py.detach(|| {
            let mut source = UParser::new().parse(&xml)?;
            source.prepare_xpath();
            sheet.transform(&source)
        })
        .map_err(xml_error_to_pyerr)
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
#[pyo3(signature = (xml, *, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
fn parse(
    py: Python<'_>,
    xml: &str,
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
) -> PyResult<Document> {
    Document::new(
        py,
        xml,
        max_depth,
        max_entity_expansion,
        namespace_aware,
        forbid_dtd,
        forbid_entities,
    )
}

/// Parse XML bytes and return a Document, with automatic encoding detection.
///
/// See ``Document.from_bytes`` for the keyword arguments that override
/// the safe parser defaults.
#[pyfunction]
#[pyo3(signature = (data, *, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
fn parse_bytes(
    py: Python<'_>,
    data: &[u8],
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
) -> PyResult<Document> {
    Document::from_bytes(
        py,
        data,
        max_depth,
        max_entity_expansion,
        namespace_aware,
        forbid_dtd,
        forbid_entities,
    )
}

/// One input to `parse_many`: text or bytes, extracted to owned data with the
/// GIL held so the worker threads never touch Python buffers.
#[derive(FromPyObject)]
enum BatchInput {
    Text(String),
    Bytes(Vec<u8>),
}

/// GIL-free failure from a `parse_many` worker thread. Converted to the
/// matching Python exception only in the attached wrap-up loop, never on a
/// detached thread (constructing a `PyErr` without the GIL is undefined
/// behavior).
enum BatchError {
    /// Byte decode failed (bad UTF-8 / UTF-16); becomes `XmlWellFormednessError`.
    Decode(String),
    /// Parse failed; mapped through `xml_error_to_pyerr` for a faithful type.
    Parse(XmlError),
    /// A worker panicked (a bug, not a normal failure); becomes `RuntimeError`.
    Panic(String),
}

/// Best-effort text from a caught panic payload, for per-item error messages.
/// `panic!("...")` payloads are `&str` or `String`; anything else gets a
/// generic description.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        format!("worker thread panicked: {}", s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("worker thread panicked: {}", s)
    } else {
        "worker thread panicked".to_string()
    }
}

/// Parse many XML documents in parallel across native threads.
///
/// Takes a list of `str` / `bytes` items and returns a list of the same
/// length, index-aligned with the input, where each slot is either a
/// `Document` or an *exception object* (not raised) describing that item's
/// parse failure -- a batch never fails wholesale; callers check
/// `isinstance(r, Exception)` per item.
///
/// The whole batch runs under one GIL release: `max_threads` worker threads
/// (default: available CPU parallelism, capped at the item count) pull items
/// off a shared index and parse them concurrently, so N threads genuinely use
/// N cores. Bytes items go through the same encoding auto-detection as
/// `parse_bytes`. The remaining keyword arguments match `parse` and apply to
/// every item.
///
/// .. warning::
///    Do not source the resource-limit kwargs from untrusted input.
///    See :class:`Document` for details.
#[pyfunction]
#[pyo3(signature = (items, *, max_threads=None, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
#[allow(clippy::too_many_arguments)]
fn parse_many(
    py: Python<'_>,
    items: Vec<BatchInput>,
    max_threads: Option<usize>,
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
) -> PyResult<Vec<Py<PyAny>>> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let n_items = items.len();
    if n_items == 0 {
        return Ok(Vec::new());
    }
    let n_threads = max_threads
        .filter(|&t| t > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
        .min(n_items);

    // The entire fan-out runs detached: workers are pure Rust over the owned
    // inputs. Results land in per-item slots (a mutex keeps the bookkeeping
    // trivially safe; one uncontended lock per item is noise next to a parse).
    // Workers never touch the Python C-API: a failure is carried as a GIL-free
    // `BatchError` and only turned into a Python exception in the wrap-up loop
    // below, once the GIL is held again.
    let results: Vec<std::sync::Mutex<Option<Result<DocWithInput, BatchError>>>> =
        (0..n_items).map(|_| std::sync::Mutex::new(None)).collect();
    py.detach(|| {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..n_threads {
                scope.spawn(|| {
                    // Each worker owns its parser; work-stealing via a shared
                    // index handles skewed input sizes (one big aggregate among
                    // many small fragments) better than static chunking.
                    let parser = build_parser(
                        max_depth,
                        max_entity_expansion,
                        namespace_aware,
                        forbid_dtd,
                        forbid_entities,
                    );
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n_items {
                            break;
                        }
                        // catch_unwind keeps a panicking item (a bug in the
                        // decode/parse path) from unwinding through
                        // thread::scope -- which would re-panic on join and
                        // take the whole batch, and with it the calling
                        // Python thread, down -- degrading it to a per-item
                        // error instead. AssertUnwindSafe is fine: on a panic
                        // the closure's only shared state (the result slot)
                        // is overwritten wholesale below.
                        let parsed: Result<DocWithInput, BatchError> =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let input = match &items[i] {
                                    BatchInput::Text(s) => s.clone(),
                                    BatchInput::Bytes(b) => {
                                        decode_xml_bytes_raw(b).map_err(BatchError::Decode)?
                                    }
                                };
                                let doc = parser
                                    .parse(&input)
                                    .map_err(BatchError::Parse)?
                                    .into_static();
                                Ok(DocWithInput { doc, input })
                            }))
                            .unwrap_or_else(|p| Err(BatchError::Panic(panic_message(p))));
                        // Tolerate a poisoned slot rather than panicking:
                        // poison only means some thread panicked while
                        // holding this lock, and the Option is replaced
                        // wholesale, so the stored value stays well-defined.
                        *results[i]
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(parsed);
                    }
                });
            }
        });
    });

    // Re-attached: wrap successes into Document pyclasses and failures into
    // exception *objects* returned in place. Never panic here (poisoned slot,
    // slot a worker somehow left unfilled): a panic in extension code aborts
    // or corrupts the interpreter, so both degrade to per-item errors.
    results
        .into_iter()
        .map(|slot| {
            let outcome = slot
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                // Unreachable now that workers catch panics, but kept as
                // defense in depth: surface it as this item's error.
                .unwrap_or_else(|| {
                    Err(BatchError::Panic(
                        "worker thread produced no result".to_string(),
                    ))
                });
            match outcome {
                Ok(dwi) => Ok(Py::new(
                    py,
                    Document {
                        inner: Arc::new(Mutex::new(dwi)),
                    },
                )?
                .into_any()),
                Err(be) => {
                    let e = match be {
                        BatchError::Decode(msg) => XmlWellFormednessError::new_err(msg),
                        BatchError::Parse(xe) => xml_error_to_pyerr(xe),
                        BatchError::Panic(msg) => PyRuntimeError::new_err(msg),
                    };
                    Ok(e.into_value(py).into_any())
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Native fetch (feature "net"): fetch_many / fetch_and_parse_many
// ---------------------------------------------------------------------------

/// The result of one URL fetch from `fetch_many` / `fetch_and_parse_many`.
#[cfg(feature = "net")]
#[pyclass(name = "FetchResult", frozen)]
struct FetchResult {
    /// The requested URL (as passed in).
    #[pyo3(get)]
    url: String,
    /// HTTP status code (200 for `file://` reads).
    #[pyo3(get)]
    status: u16,
    /// Canonical reason phrase for the status, e.g. "OK".
    #[pyo3(get)]
    reason: String,
    /// Response headers, keys lowercased. Empty for `file://` reads.
    headers: Vec<(String, String)>,
    /// Raw response body bytes (transparently gunzipped by the client).
    body: Vec<u8>,
    /// Wall-clock milliseconds spent on this fetch (including retries).
    #[pyo3(get)]
    elapsed_ms: f64,
}

#[cfg(feature = "net")]
#[pymethods]
impl FetchResult {
    /// Response headers as a dict with lowercased keys.
    #[getter]
    fn headers(&self) -> std::collections::HashMap<String, String> {
        self.headers.iter().cloned().collect()
    }

    /// Raw response body bytes.
    #[getter]
    fn body<'py>(&self, py: Python<'py>) -> Bound<'py, pyo3::types::PyBytes> {
        pyo3::types::PyBytes::new(py, &self.body)
    }

    fn __repr__(&self) -> String {
        format!(
            "FetchResult(url={:?}, status={}, {} bytes)",
            self.url,
            self.status,
            self.body.len()
        )
    }
}

/// Plain-Rust fetch output built on worker threads (no Python objects there).
#[cfg(feature = "net")]
struct FetchOut {
    url: String,
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    elapsed_ms: f64,
}

/// Shared fetch knobs, extracted from Python kwargs with the GIL held.
#[cfg(feature = "net")]
struct FetchOpts {
    timeout: f64,
    connect_timeout: f64,
    verify_tls: bool,
    follow_redirects: bool,
    retries: u32,
    retry_backoff: f64,
    user_agent: String,
    extra_headers: Vec<(String, String)>,
    max_body: u64,
}

/// Upper bound for the user-supplied second-valued knobs (about 31 years):
/// far beyond any sane timeout, but small enough that every downstream
/// `Duration::from_secs_f64` stays well inside its panic-free range.
#[cfg(feature = "net")]
const MAX_FETCH_SECONDS: f64 = 1e9;

/// Validate the user-controlled float knobs at the Python entrypoint, with
/// the GIL held. `timeout` / `connect_timeout` / `retry_backoff` all flow
/// into `Duration::from_secs_f64` (via `build_agent` / `fetch_one`), which
/// panics on negative, NaN, infinite, or absurdly large values -- and a
/// panic in extension code must never be reachable from Python arguments.
#[cfg(feature = "net")]
fn validate_fetch_floats(timeout: f64, connect_timeout: f64, retry_backoff: f64) -> PyResult<()> {
    for (name, v) in [
        ("timeout", timeout),
        ("connect_timeout", connect_timeout),
        ("retry_backoff", retry_backoff),
    ] {
        if !v.is_finite() || v < 0.0 || v > MAX_FETCH_SECONDS {
            return Err(PyValueError::new_err(format!(
                "{} must be a finite number of seconds in 0..={:e} (got {})",
                name, MAX_FETCH_SECONDS, v
            )));
        }
    }
    Ok(())
}

/// Build the shared ureq Agent for a batch (Send + Sync; shared by
/// reference across the scoped worker threads).
#[cfg(feature = "net")]
fn build_agent(opts: &FetchOpts) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .disable_verification(!opts.verify_tls)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(std::time::Duration::from_secs_f64(opts.timeout)))
        .timeout_connect(Some(std::time::Duration::from_secs_f64(
            opts.connect_timeout,
        )))
        .max_redirects(if opts.follow_redirects { 10 } else { 0 })
        .user_agent(opts.user_agent.as_str())
        // pyFF-style callers inspect the status themselves (e.g. fall back to
        // a local copy on non-2xx), so a non-2xx response is a result, not an
        // error.
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// Fetch one URL (with retries) on a worker thread. `file://` URLs read the
/// local filesystem directly, covering pyFF's exploded directory sources.
#[cfg(feature = "net")]
fn fetch_one(agent: &ureq::Agent, url: &str, opts: &FetchOpts) -> Result<FetchOut, String> {
    let start = std::time::Instant::now();
    if let Some(path) = url.strip_prefix("file://") {
        let body = read_file_url_body(url, path, opts.max_body)?;
        return Ok(FetchOut {
            url: url.to_string(),
            status: 200,
            reason: "OK".to_string(),
            headers: Vec::new(),
            body,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        });
    }
    let mut attempt = 0u32;
    loop {
        let result = (|| -> Result<FetchOut, String> {
            let mut req = agent.get(url);
            for (k, v) in &opts.extra_headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let mut resp = req.call().map_err(|e| format!("{}: {}", url, e))?;
            let status = resp.status();
            let headers = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_ascii_lowercase(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect();
            let body = resp
                .body_mut()
                .with_config()
                .limit(opts.max_body)
                .read_to_vec()
                .map_err(|e| format!("{}: {}", url, e))?;
            Ok(FetchOut {
                url: url.to_string(),
                status: status.as_u16(),
                reason: status.canonical_reason().unwrap_or("").to_string(),
                headers,
                body,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            })
        })();
        match result {
            Ok(out) => return Ok(out),
            Err(e) if attempt < opts.retries => {
                // Exponential backoff, matching the requests Retry(backoff)
                // curve pyFF used: backoff * 2^attempt seconds. The exponent
                // is clamped and the sleep capped so a large `retries` value
                // can neither overflow the exponentiation nor reach
                // Duration::from_secs_f64 with a non-finite/oversized value
                // (which panics). retry_backoff itself is validated finite
                // and non-negative at the Python entrypoint.
                const MAX_BACKOFF_SECS: f64 = 300.0;
                let factor = 2f64.powi(attempt.min(32) as i32);
                let sleep_secs = (opts.retry_backoff * factor).min(MAX_BACKOFF_SECS);
                std::thread::sleep(std::time::Duration::from_secs_f64(sleep_secs));
                attempt += 1;
                let _ = e;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(feature = "net")]
fn read_file_url_body(url: &str, path: &str, max_body: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|e| format!("{}: {}", url, e))?;
    let mut body = Vec::new();
    if max_body == u64::MAX {
        let mut reader = file;
        reader
            .read_to_end(&mut body)
            .map_err(|e| format!("{}: {}", url, e))?;
    } else {
        let mut reader = file.take(max_body + 1);
        reader
            .read_to_end(&mut body)
            .map_err(|e| format!("{}: {}", url, e))?;
        if body.len() as u64 > max_body {
            return Err(format!(
                "{}: body exceeds max_body of {} bytes",
                url, max_body
            ));
        }
    }
    Ok(body)
}

/// Run the shared scoped-thread fan-out for a URL batch, entirely detached.
#[cfg(feature = "net")]
fn fetch_batch<R: Send>(
    py: Python<'_>,
    urls: &[String],
    max_threads: Option<usize>,
    opts: &FetchOpts,
    per_item: impl Fn(Result<FetchOut, String>) -> R + Sync,
) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n_items = urls.len();
    // Empty batch: nothing to fetch, so skip the agent build, the GIL detach
    // and the thread::scope setup entirely.
    if n_items == 0 {
        return Vec::new();
    }
    let n_threads = max_threads
        .filter(|&t| t > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
        .min(n_items);
    let agent = build_agent(opts);
    let results: Vec<std::sync::Mutex<Option<R>>> =
        (0..n_items).map(|_| std::sync::Mutex::new(None)).collect();
    py.detach(|| {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..n_threads {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n_items {
                        break;
                    }
                    // catch_unwind: a panic in the fetch or the caller's
                    // per_item mapping (a bug, not an I/O failure) degrades
                    // to that item's error via per_item's own Err arm instead
                    // of unwinding through thread::scope and taking the whole
                    // batch -- and the calling Python thread -- down.
                    // AssertUnwindSafe is fine: the only shared state (the
                    // result slot) is overwritten wholesale below.
                    let val = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        per_item(fetch_one(&agent, &urls[i], opts))
                    }))
                    .unwrap_or_else(|p| per_item(Err(panic_message(p))));
                    // Tolerate a poisoned slot rather than panicking: poison
                    // only means some thread panicked while holding this
                    // lock, and the Option is replaced wholesale.
                    *results[i]
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(val);
                });
            }
        });
    });
    // Never panic in the wrap-up (this runs attached, on the calling Python
    // thread): tolerate poison, and turn a slot a worker somehow left
    // unfilled into that item's error through the caller's Err mapping.
    // The unfilled case is unreachable now that workers catch panics, but
    // kept as defense in depth.
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .unwrap_or_else(|| per_item(Err("worker thread produced no result".to_string())))
        })
        .collect()
}

/// Fetch many URLs concurrently in native threads with the GIL released.
///
/// Returns a list index-aligned with ``urls`` where each slot is either a
/// :class:`FetchResult` or an exception object (not raised) -- a batch never
/// fails wholesale. ``file://`` URLs are read from the local filesystem.
/// Non-2xx HTTP responses are returned as results (check ``.status``), not
/// errors, matching how pyFF inspects statuses itself.
///
/// ``verify_tls=False`` disables TLS certificate verification (pyFF fetches
/// federation metadata this way and verifies XML signatures instead).
///
/// Each response body is buffered fully in memory, capped at
/// ``max_body`` bytes (default 128 MiB -- comfortably above the largest
/// federation metadata aggregates while bounding what an untrusted or
/// misbehaving URL can allocate). A larger body fails with a per-item
/// error; callers who genuinely expect bigger payloads opt in via
/// ``max_body=``.
#[cfg(feature = "net")]
#[pyfunction]
#[pyo3(signature = (urls, *, max_threads=None, timeout=30.0, connect_timeout=10.0, verify_tls=true, follow_redirects=true, retries=0, retry_backoff=0.5, user_agent=None, extra_headers=None, max_body=134_217_728))]
#[allow(clippy::too_many_arguments)]
fn fetch_many(
    py: Python<'_>,
    urls: Vec<String>,
    max_threads: Option<usize>,
    timeout: f64,
    connect_timeout: f64,
    verify_tls: bool,
    follow_redirects: bool,
    retries: u32,
    retry_backoff: f64,
    user_agent: Option<String>,
    extra_headers: Option<std::collections::HashMap<String, String>>,
    max_body: u64,
) -> PyResult<Vec<Py<PyAny>>> {
    validate_fetch_floats(timeout, connect_timeout, retry_backoff)?;
    let opts = FetchOpts {
        timeout,
        connect_timeout,
        verify_tls,
        follow_redirects,
        retries,
        retry_backoff,
        user_agent: user_agent
            .unwrap_or_else(|| format!("pyuppsala/{}", env!("CARGO_PKG_VERSION"))),
        extra_headers: extra_headers
            .map(|h| h.into_iter().collect())
            .unwrap_or_default(),
        max_body,
    };
    let outs = fetch_batch(py, &urls, max_threads, &opts, |r| r);
    outs.into_iter()
        .map(|r| match r {
            Ok(o) => Ok(Py::new(
                py,
                FetchResult {
                    url: o.url,
                    status: o.status,
                    reason: o.reason,
                    headers: o.headers,
                    body: o.body,
                    elapsed_ms: o.elapsed_ms,
                },
            )?
            .into_any()),
            Err(msg) => Ok(PyRuntimeError::new_err(msg).into_value(py).into_any()),
        })
        .collect()
}

/// Fetch many URLs and parse each response as XML, all in native threads
/// with the GIL released (the parse happens on the same worker that fetched,
/// so the response bytes never cross the FFI boundary).
///
/// Returns a list index-aligned with ``urls``: each slot is a
/// ``(FetchResult, Document)`` tuple, or an exception object for a fetch,
/// non-2xx status, or parse failure of that item.
///
/// Bodies are buffered fully in memory and capped at ``max_body`` bytes
/// (default 128 MiB); see :func:`fetch_many` for the rationale and the
/// opt-in for larger payloads.
#[cfg(feature = "net")]
#[pyfunction]
#[pyo3(signature = (urls, *, max_threads=None, timeout=30.0, connect_timeout=10.0, verify_tls=true, follow_redirects=true, retries=0, retry_backoff=0.5, user_agent=None, extra_headers=None, max_body=134_217_728, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
#[allow(clippy::too_many_arguments)]
fn fetch_and_parse_many(
    py: Python<'_>,
    urls: Vec<String>,
    max_threads: Option<usize>,
    timeout: f64,
    connect_timeout: f64,
    verify_tls: bool,
    follow_redirects: bool,
    retries: u32,
    retry_backoff: f64,
    user_agent: Option<String>,
    extra_headers: Option<std::collections::HashMap<String, String>>,
    max_body: u64,
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
) -> PyResult<Vec<Py<PyAny>>> {
    validate_fetch_floats(timeout, connect_timeout, retry_backoff)?;
    let opts = FetchOpts {
        timeout,
        connect_timeout,
        verify_tls,
        follow_redirects,
        retries,
        retry_backoff,
        user_agent: user_agent
            .unwrap_or_else(|| format!("pyuppsala/{}", env!("CARGO_PKG_VERSION"))),
        extra_headers: extra_headers
            .map(|h| h.into_iter().collect())
            .unwrap_or_default(),
        max_body,
    };
    // Fetch AND parse inside the worker: the response bytes stay in Rust.
    // Errors are plain strings (PyErr construction is deferred to the
    // attached wrap-up below).
    let outs = fetch_batch(py, &urls, max_threads, &opts, |r| {
        r.and_then(|o| {
            if !(200..300).contains(&o.status) {
                return Err(format!("{}: HTTP status {}", o.url, o.status));
            }
            let parser = build_parser(
                max_depth,
                max_entity_expansion,
                namespace_aware,
                forbid_dtd,
                forbid_entities,
            );
            // Plain-string errors only in this detached worker: the GIL-free
            // decoder and XmlError's Display are pure Rust, so no PyErr is ever
            // constructed off the GIL (that would touch the Python C-API).
            // Keep the decoder's detailed message (invalid UTF-8 vs
            // odd-length UTF-16, etc.) -- per-item failures are much easier
            // to debug with the real cause attached to the URL.
            let input = decode_xml_bytes_raw(&o.body).map_err(|e| format!("{}: {}", o.url, e))?;
            let doc = parser
                .parse(&input)
                .map_err(|e| format!("{}: {}", o.url, e))?
                .into_static();
            Ok((o, DocWithInput { doc, input }))
        })
    });
    outs.into_iter()
        .map(|r| match r {
            Ok((o, dwi)) => {
                let fr = Py::new(
                    py,
                    FetchResult {
                        url: o.url,
                        status: o.status,
                        reason: o.reason,
                        headers: o.headers,
                        body: o.body,
                        elapsed_ms: o.elapsed_ms,
                    },
                )?;
                let doc = Py::new(
                    py,
                    Document {
                        inner: Arc::new(Mutex::new(dwi)),
                    },
                )?;
                Ok((fr, doc).into_pyobject(py)?.into_any().unbind())
            }
            Err(msg) => Ok(PyRuntimeError::new_err(msg).into_value(py).into_any()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_parser(
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
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
    if let Some(true) = forbid_dtd {
        parser = parser.with_forbid_dtd(true);
    }
    if let Some(true) = forbid_entities {
        parser = parser.with_forbid_entities(true);
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
    // GIL-holding wrapper: turns the plain-String decode error into the
    // Python exception. Callers inside `py.detach` must use the `_raw` form
    // instead (see `decode_xml_bytes_raw`).
    decode_xml_bytes_raw(data).map_err(XmlWellFormednessError::new_err)
}

/// GIL-free core of [`decode_xml_bytes`]: BOM sniffing plus UTF-8/UTF-16 decode,
/// returning a plain `String` error. This is the form that detached worker
/// threads (`py.detach`) must call: constructing a `PyErr` there touches the
/// Python C-API without the GIL, which is undefined behavior. The caller
/// re-attaches and converts the error into a Python exception.
fn decode_xml_bytes_raw(data: &[u8]) -> Result<String, String> {
    if data.len() < 2 {
        // Too short for BOM detection - assume UTF-8.
        return decode_utf8_raw(data);
    }

    // Byte-order mark detection.
    if data[0] == 0xFF && data[1] == 0xFE {
        return decode_utf16_raw(&data[2..], false); // UTF-16 LE BOM
    }
    if data[0] == 0xFE && data[1] == 0xFF {
        return decode_utf16_raw(&data[2..], true); // UTF-16 BE BOM
    }
    if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        // UTF-8 BOM - strip it and decode as UTF-8.
        return decode_utf8_raw(&data[3..]);
    }

    // No BOM - check for UTF-16 without BOM (XML spec Appendix F).
    if data[0] == 0x00 && data[1] == 0x3C {
        return decode_utf16_raw(data, true); // UTF-16 BE without BOM
    }
    if data[0] == 0x3C && data[1] == 0x00 {
        return decode_utf16_raw(data, false); // UTF-16 LE without BOM
    }

    // Default: UTF-8.
    decode_utf8_raw(data)
}

/// Validate UTF-8 bytes and copy them into a String. Borrows the slice for
/// validation (`std::str::from_utf8`) so there is no intermediate `Vec<u8>`
/// allocation on the common UTF-8 path - only the final owned copy. GIL-free.
fn decode_utf8_raw(bytes: &[u8]) -> Result<String, String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|e| format!("1:1: Invalid UTF-8: {}", e))
}

/// Decode UTF-16 bytes (big- or little-endian) to a String. An odd-length
/// input is rejected as malformed rather than silently dropping the trailing
/// byte (which could truncate invalid UTF-16 into superficially valid text).
/// GIL-free.
fn decode_utf16_raw(bytes: &[u8], big_endian: bool) -> Result<String, String> {
    let endian = if big_endian { "BE" } else { "LE" };
    if !bytes.len().is_multiple_of(2) {
        return Err(format!(
            "1:1: Invalid UTF-16 {}: odd number of bytes",
            endian
        ));
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
    String::from_utf16(&code_units).map_err(|e| format!("1:1: Invalid UTF-16 {}: {}", endian, e))
}

fn make_write_options(
    indent: Option<&str>,
    expand_empty_elements: bool,
    include_doctype: bool,
) -> XmlWriteOptions {
    let mut opts = match indent {
        Some(s) => XmlWriteOptions::pretty(s),
        None => XmlWriteOptions::compact(),
    };
    if expand_empty_elements {
        opts = opts.with_expand_empty_elements(true);
    }
    if include_doctype {
        // Opt-in serialization of the preserved `<!DOCTYPE ...>` declaration.
        // Disabled by default so a parsed DTD is not handed to downstream
        // processors unless the caller deliberately opts into round-tripping.
        opts = opts.with_doctype(true);
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
    m.add_class::<ElementBase>()?;
    m.add_class::<DocHolderBase>()?;
    m.add_class::<QName>()?;
    m.add_class::<Attribute>()?;
    m.add_class::<XPathEvaluator>()?;
    m.add_class::<XsdValidator>()?;
    m.add_class::<ValidationErrorPy>()?;
    m.add_class::<XmlWriter>()?;
    m.add_class::<XsdRegex>()?;
    m.add_class::<Xslt>()?;

    // Functions
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(parse_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(parse_many, m)?)?;
    #[cfg(feature = "net")]
    {
        m.add_class::<FetchResult>()?;
        m.add_function(wrap_pyfunction!(fetch_many, m)?)?;
        m.add_function(wrap_pyfunction!(fetch_and_parse_many, m)?)?;
    }
    m.add_function(wrap_pyfunction!(_register_element_helpers, m)?)?;
    m.add_function(wrap_pyfunction!(_register_element_type, m)?)?;

    // Default resource-limit constants (uppsala 0.4.0 / 0.5.0 hardening)
    m.add("DEFAULT_MAX_DEPTH", DEFAULT_MAX_DEPTH)?;
    m.add("DEFAULT_MAX_ENTITY_EXPANSION", DEFAULT_MAX_ENTITY_EXPANSION)?;
    // Entity-nesting cap added in uppsala 0.5.0 (enforced internally; no builder).
    m.add("DEFAULT_MAX_ENTITY_DEPTH", DEFAULT_MAX_ENTITY_DEPTH)?;
    m.add(
        "DEFAULT_MAX_XPATH_DEPTH",
        uppsala::xpath::DEFAULT_MAX_XPATH_DEPTH,
    )?;
    // Per-evaluation XPath node-visit budget added in uppsala 0.5.0.
    m.add(
        "DEFAULT_MAX_XPATH_NODE_VISITS",
        uppsala::xpath::DEFAULT_MAX_XPATH_NODE_VISITS,
    )?;
    m.add(
        "DEFAULT_MAX_REGEX_GROUP_DEPTH",
        uppsala::xsd_regex::DEFAULT_MAX_REGEX_GROUP_DEPTH,
    )?;
    m.add(
        "DEFAULT_MAX_REGEX_STEPS",
        uppsala::xsd_regex::DEFAULT_MAX_REGEX_STEPS,
    )?;
    // XSLT template-activation recursion cap (uppsala XSLT 1.0 engine).
    m.add(
        "DEFAULT_MAX_XSLT_DEPTH",
        uppsala::xslt::DEFAULT_MAX_XSLT_DEPTH,
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
