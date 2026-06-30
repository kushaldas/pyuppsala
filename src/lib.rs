use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

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

fn writer_attr_refs<'a>(
    attrs: &'a Option<Vec<(String, String)>>,
) -> PyResult<Vec<(&'a str, &'a str)>> {
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
        // Node-level (fragment) serialization never emits a DOCTYPE, so
        // `include_doctype` is fixed to false here. DOCTYPE round-tripping is
        // only meaningful for whole-document serialization (see
        // `Document.to_xml_with_options`).
        let opts = make_write_options(indent, expand_empty_elements, false);
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
        while let Some(id) = self.stack.pop() {
            // Push this node's children in reverse document order so the first
            // child is popped next (pre-order). Done before the match check so
            // we descend into matching nodes too.
            let start = self.stack.len();
            let mut child = guard.doc.first_child(id);
            while let Some(cid) = child {
                self.stack.push(cid);
                child = guard.doc.next_sibling(cid);
            }
            self.stack[start..].reverse();

            let matched = match &self.filter {
                DescFilter::All => matches!(
                    guard.doc.node_kind(id),
                    Some(NodeKind::Element(_))
                        | Some(NodeKind::Comment(_))
                        | Some(NodeKind::ProcessingInstruction(_))
                ),
                DescFilter::Elements => {
                    matches!(guard.doc.node_kind(id), Some(NodeKind::Element(_)))
                }
                DescFilter::Named { ns, local } => {
                    matches!(guard.doc.element(id), Some(e) if e.name.matches(ns.as_deref(), local))
                }
            };
            if matched {
                return Ok(Some(Node {
                    doc: Arc::clone(&self.doc),
                    id,
                }));
            }
        }
        Ok(None)
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
        xml: &str,
        max_depth: Option<u32>,
        max_entity_expansion: Option<usize>,
        namespace_aware: Option<bool>,
        forbid_dtd: Option<bool>,
        forbid_entities: Option<bool>,
    ) -> PyResult<Self> {
        let input = xml.to_string();
        let parser = build_parser(
            max_depth,
            max_entity_expansion,
            namespace_aware,
            forbid_dtd,
            forbid_entities,
        );
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
    #[pyo3(signature = (data, *, max_depth=None, max_entity_expansion=None, namespace_aware=None, forbid_dtd=None, forbid_entities=None))]
    fn from_bytes(
        data: &[u8],
        max_depth: Option<u32>,
        max_entity_expansion: Option<usize>,
        namespace_aware: Option<bool>,
        forbid_dtd: Option<bool>,
        forbid_entities: Option<bool>,
    ) -> PyResult<Document> {
        let input = decode_xml_bytes(data)?;
        let parser = build_parser(
            max_depth,
            max_entity_expansion,
            namespace_aware,
            forbid_dtd,
            forbid_entities,
        );
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
    fn append_child(&self, parent: &Node, child: &Node) -> PyResult<()> {
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
    /// Locks the source document first, then this one. The source must be a
    /// different `Document`; importing from the same document raises ValueError
    /// (use the move/detach path instead).
    fn import_subtree(&self, source: &Node) -> PyResult<Node> {
        if Arc::ptr_eq(&self.inner, &source.doc) {
            return Err(PyValueError::new_err(
                "import_subtree requires a node from a different Document",
            ));
        }
        let src_guard = source
            .doc
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut dst_guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
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
    /// namespace, or declaring the `xmlns` namespace).
    #[pyo3(signature = (node, prefix, uri))]
    fn set_namespace_declaration(
        &self,
        node: &Node,
        prefix: Option<&str>,
        uri: &str,
    ) -> PyResult<()> {
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
    ///     include_doctype: If True, serialize the preserved ``<!DOCTYPE ...>``
    ///         declaration (if the document had one) ahead of the root element.
    ///         Defaults to False so a parsed DTD is not re-emitted unless the
    ///         caller deliberately opts into round-tripping it.
    #[pyo3(signature = (indent=None, expand_empty_elements=false, include_doctype=false))]
    fn to_xml_with_options(
        &self,
        indent: Option<&str>,
        expand_empty_elements: bool,
        include_doctype: bool,
    ) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let opts = make_write_options(indent, expand_empty_elements, include_doctype);
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
    fn new(stylesheet_xml: &str, exslt: bool, max_depth: Option<u32>) -> PyResult<Self> {
        let style_doc = UParser::new()
            .parse(stylesheet_xml)
            .map_err(xml_error_to_pyerr)?;
        let mut sheet =
            uppsala::xslt::Stylesheet::compile(&style_doc).map_err(xml_error_to_pyerr)?;
        if let Some(d) = max_depth {
            sheet = sheet.set_max_depth(d);
        }
        sheet = sheet.with_exslt(exslt);
        Ok(Xslt { inner: sheet })
    }

    /// Apply the stylesheet to a source XML string, returning the serialized
    /// result. The source is parsed and prepared for XPath internally.
    fn transform(&self, source_xml: &str) -> PyResult<String> {
        let mut source = UParser::new()
            .parse(source_xml)
            .map_err(xml_error_to_pyerr)?;
        source.prepare_xpath();
        self.inner.transform(&source).map_err(xml_error_to_pyerr)
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
    xml: &str,
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
) -> PyResult<Document> {
    Document::new(
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
    data: &[u8],
    max_depth: Option<u32>,
    max_entity_expansion: Option<usize>,
    namespace_aware: Option<bool>,
    forbid_dtd: Option<bool>,
    forbid_entities: Option<bool>,
) -> PyResult<Document> {
    Document::from_bytes(
        data,
        max_depth,
        max_entity_expansion,
        namespace_aware,
        forbid_dtd,
        forbid_entities,
    )
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
    m.add("DEFAULT_MAX_XSLT_DEPTH", uppsala::xslt::DEFAULT_MAX_XSLT_DEPTH)?;

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
