"""An ``lxml.etree``-compatible API layered on the Uppsala XML engine.

This module lets code written for ``lxml.etree`` run on pyuppsala's secure,
pure-Rust parser with minimal changes::

    from pyuppsala import etree
    root = etree.fromstring("<a><b>hi</b></a>")
    print(root.find("b").text)

Elements are live views over a backing native ``Document`` (mirroring lxml,
where ``_Element`` objects are views over a libxml2 tree). Each standalone tree
owns one native document; cross-tree moves deep-copy the subtree into the target
document and preserve Python object identity via a per-document proxy cache.

See ``docs/etree.rst`` for the supported/unsupported feature matrix.
"""

from __future__ import annotations

import os
import re
import sys
from weakref import ref as _weakref

from . import _elementpath as ElementPath
from . import _pyuppsala as _u

__all__ = [
    # Factories & node types
    "Element",
    "SubElement",
    "Comment",
    "ProcessingInstruction",
    "PI",
    "QName",
    "ElementTree",
    "DocInfo",
    # I/O
    "fromstring",
    "fromstringlist",
    "XML",
    "parse",
    "tostring",
    "tounicode",
    "dump",
    "indent",
    "iselement",
    # Search
    "XPath",
    "ETXPath",
    "XPathEvaluator",
    # Parser & validation
    "XMLParser",
    "register_namespace",
    "XMLSchema",
    # Transformation
    "XSLT",
    # Exceptions
    "LxmlError",
    "Error",
    "XMLSyntaxError",
    "ParseError",
    "XPathError",
    "XPathEvalError",
    "XPathSyntaxError",
    "DocumentInvalid",
    "XMLSchemaParseError",
    "XSLTError",
    "XSLTParseError",
    "XSLTApplyError",
    "XIncludeError",
]


# Per-evaluation node-visit budget applied by :meth:`_Element.xpath`. lxml's
# ``.xpath()`` has no such cap, so the default is effectively unbounded to match
# it (and to allow XPath over large *trusted* documents, e.g. a SAML aggregate
# with thousands of entities, which would otherwise exceed the native
# evaluator's much lower default). Applications that evaluate XPath over
# UNTRUSTED input can lower this module attribute to restore an anti-DoS bound,
# e.g. ``etree.MAX_XPATH_NODE_VISITS = pyuppsala.DEFAULT_MAX_XPATH_NODE_VISITS``.
MAX_XPATH_NODE_VISITS = sys.maxsize


# ---------------------------------------------------------------------------
# Exceptions (lxml-named hierarchy, mapping pyuppsala exceptions underneath)
# ---------------------------------------------------------------------------


class LxmlError(Exception):
    """Base class for all exceptions raised by pyuppsala.etree."""


Error = LxmlError


class XMLSyntaxError(LxmlError, SyntaxError):
    """Raised for XML parsing / well-formedness errors."""


# ElementTree code commonly catches ``ParseError``.
ParseError = XMLSyntaxError


class XPathError(LxmlError):
    """Base class for XPath errors."""


class XPathEvalError(XPathError):
    """Raised when an XPath expression fails to evaluate."""


class XPathSyntaxError(XPathError, SyntaxError):
    """Raised when an XPath expression is malformed."""


class DocumentInvalid(LxmlError):
    """Raised by ``XMLSchema.assertValid`` when a document fails validation.

    Carries an ``error_log`` list of validation errors (lxml-compatible);
    populated by ``assertValid`` and otherwise an empty list.
    """

    def __init__(self, *args):
        super().__init__(*args)
        # Per-instance so appends by one caller never leak onto other
        # DocumentInvalid objects (a class-level list would be shared).
        self.error_log = []


class XMLSchemaParseError(LxmlError):
    """Raised when an XSD schema cannot be built."""


class XSLTError(LxmlError):
    """Base class for XSLT errors."""


class XSLTParseError(XSLTError):
    """Raised when an XSLT stylesheet cannot be compiled."""


class XSLTApplyError(XSLTError):
    """Raised when applying a compiled XSLT stylesheet fails."""


class XIncludeError(LxmlError):
    """Raised when XInclude processing fails (and no fallback applies)."""


# ---------------------------------------------------------------------------
# Namespace registry and Clark-notation helpers
# ---------------------------------------------------------------------------

_namespace_map = {
    "http://www.w3.org/XML/1998/namespace": "xml",
    "http://www.w3.org/1999/xhtml": "html",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#": "rdf",
    "http://schemas.xmlsoap.org/wsdl/": "wsdl",
    "http://www.w3.org/2001/XMLSchema": "xs",
    "http://www.w3.org/2001/XMLSchema-instance": "xsi",
}

# XML construction APIs write names directly into serialized markup.  Validate
# local names and namespace prefixes before they enter the native DOM so callers
# cannot smuggle tag delimiters, quotes, or whitespace into element/attribute
# positions.  The etree layer uses NCName because namespace URI and prefix are
# tracked separately.
# The exact XML 1.0 NCNameStartChar / NCNameChar code-point sets, kept in sync
# with is_xml_name_start / is_xml_name_char in src/lib.rs. Spelled out range by
# range (rather than a single \xC0-\uD7FF span) so disallowed code points such
# as U+00D7, U+00F7, and the combining marks at U+0300..U+036F are excluded from
# the start set, and only valid names round-trip through other XML parsers.
_NCNAME_START = (
    r"A-Z_a-z"
    r"\xC0-\xD6\xD8-\xF6\xF8-\u02FF\u0370-\u037D\u037F-\u1FFF"
    r"\u200C-\u200D\u2070-\u218F\u2C00-\u2FEF\u3001-\uD7FF"
    r"\uF900-\uFDCF\uFDF0-\uFFFD\U00010000-\U000EFFFF"
)
_NCNAME_CHAR = _NCNAME_START + r"\-.0-9\xB7\u0300-\u036F\u203F-\u2040"
_NCNAME_RE = re.compile(r"^[%s][%s]*$" % (_NCNAME_START, _NCNAME_CHAR))


def _validate_ncname(value, what):
    """Validate an XML NCName and return it.

    etree represents namespaces in Clark notation or as separate
    ``namespace_uri``/``prefix`` fields, so element local names, attribute local
    names, and prefixes must be NCNames (no embedded colon).
    """
    if not isinstance(value, str):
        raise TypeError("%s name must be a string" % what)
    if not _NCNAME_RE.match(value):
        raise ValueError("Invalid %s name %r" % (what, value))
    return value


def _validate_prefix(prefix):
    """Validate a namespace prefix.

    ``None`` denotes the default namespace.  An empty string is normalized to
    ``None`` for compatibility with callers that use ``""`` to mean default.
    """
    if prefix in (None, ""):
        return None
    return _validate_ncname(prefix, "namespace prefix")


def register_namespace(prefix, uri):
    """Register a prefix -> URI mapping used when serializing built trees."""
    if not isinstance(prefix, str) or not isinstance(uri, str):
        raise TypeError("prefix and uri must be strings")
    prefix = _validate_prefix(prefix)
    if prefix is None:
        raise ValueError("Default namespaces must be declared with nsmap={None: uri}")
    if prefix in ("xml", "xmlns"):
        # These prefixes are bound by the XML Namespaces spec; rebinding them
        # would clobber the standard xml binding or produce invalid markup.
        raise ValueError("Prefix %r is reserved." % prefix)
    if re.match(r"ns\d+$", prefix):
        raise ValueError("Prefixes of the form ns<N> are reserved.")
    # Drop any existing mapping for this uri or prefix, then register.
    for u, p in list(_namespace_map.items()):
        if u == uri or p == prefix:
            del _namespace_map[u]
    _namespace_map[uri] = prefix


def _tag_split(tag):
    """Split a tag string into ``(namespace_uri_or_None, local_name)``."""
    if not isinstance(tag, str):
        raise TypeError("Invalid tag name %r" % (tag,))
    if tag and tag[0] == "{":
        uri, brace, local = tag[1:].partition("}")
        if not brace:
            raise ValueError("Invalid tag name %r" % tag)
        return (uri or None), _validate_ncname(local, "tag")
    return None, _validate_ncname(tag, "tag")


def _make_clark(namespace_uri, local):
    if namespace_uri:
        return "{%s}%s" % (namespace_uri, local)
    return local


def _clark_of(tag):
    if isinstance(tag, QName):
        return tag.text
    return tag


def _split_key(key):
    """Split a tag/attribute key (str or QName) into ``(ns_or_None, local)``."""
    if isinstance(key, QName):
        return key.namespace, key.localname
    return _tag_split(key)


def _validate_nsmap(nsmap):
    """Return a validated namespace map with default prefixes normalized.

    lxml uses ``None`` for the default namespace in ``nsmap``.  We also accept
    ``""`` as input and normalize it to ``None`` before passing declarations to
    the native layer.
    """
    if nsmap is None:
        return None
    normalized = {}
    for pfx, uri in nsmap.items():
        if not isinstance(uri, str):
            raise TypeError("namespace URI must be a string")
        normalized[_validate_prefix(pfx)] = uri
    return normalized


def _prefix_for_ns(ns, nsmap):
    """Pick a serialization prefix for ``ns`` from ``nsmap``/registry, or None."""
    if nsmap:
        has_default = False
        for pfx, uri in nsmap.items():
            if uri == ns:
                if pfx in (None, ""):
                    has_default = True
                else:
                    return pfx
        if has_default:
            return None
    return _namespace_map.get(ns)


# ---------------------------------------------------------------------------
# QName
# ---------------------------------------------------------------------------


class QName:
    """A qualified XML name, compatible with ``lxml.etree.QName``."""

    def __init__(self, text_or_uri_or_element, tag=None):
        """Build from ``QName("{uri}local")``, ``QName(uri, local)``, or an element.

        With both arguments, the first is the namespace URI and the second the
        local name; with one argument it may be Clark notation, a bare local
        name, an existing QName, or an element (whose tag is used).
        """
        if tag is not None:
            uri = text_or_uri_or_element
            if isinstance(uri, QName):
                uri = uri.namespace
            self.text = _make_clark(uri, tag) if uri else tag
        else:
            value = text_or_uri_or_element
            if isinstance(value, _Element):
                value = value.tag
            if isinstance(value, QName):
                value = value.text
            self.text = value
        ns, local = _tag_split(self.text)
        self.namespace = ns
        self.localname = local

    def __str__(self):
        return self.text

    def __repr__(self):
        return "QName(%r)" % self.text

    def __hash__(self):
        return hash(self.text)

    def __eq__(self, other):
        if isinstance(other, QName):
            return self.text == other.text
        if isinstance(other, str):
            return self.text == other
        return NotImplemented

    def __ne__(self, other):
        result = self.__eq__(other)
        if result is NotImplemented:
            return result
        return not result


# ---------------------------------------------------------------------------
# Document holder + proxy cache
# ---------------------------------------------------------------------------

_CONTENT_KINDS = ("element", "comment", "processing_instruction")
_TEXT_KINDS = ("text", "cdata")


class _DocHolder:
    """Owns one native Document and an identity-stable proxy cache."""

    __slots__ = ("doc", "_proxies", "_ns_counter", "base_url", "_sweep_at", "__weakref__")

    def __init__(self, doc):
        self.doc = doc
        # node_id -> weakref.ref(_Element), *without* a death callback. A plain
        # dict of callback-free weakrefs is markedly cheaper than
        # WeakValueDictionary on the hot tree-walk path: pyFF's iter()/with_tree
        # walks create a proxy per node and drop it immediately (no re-access),
        # so WeakValueDictionary paid, per node, both a KeyedRef-with-callback
        # creation *and* the callback firing on the proxy's near-instant death
        # (~25% of with_tree wall time was this churn). Callback-free refs skip
        # the death callback entirely; dead entries are reclaimed by the bounded
        # lazy sweep below instead.
        self._proxies = {}
        self._ns_counter = 0
        # Size at which proxy() next sweeps dead refs. Re-armed after each sweep
        # to a small multiple of the live count, so during a transient walk the
        # cache holds at most ~this many tombstones (bounded memory) and the
        # total sweep work stays O(nodes visited).
        self._sweep_at = 256
        # Base URL/path for resolving relative XInclude hrefs (set by parse()).
        self.base_url = None

    def proxy(self, node):
        """Return the identity-stable ``_Element`` wrapper for ``node``.

        Looks the node up by its stable ``node_id`` in the per-document cache so
        that repeated lookups of the same underlying node return the *same*
        Python object (``root[0] is root[0]``), matching lxml. ``node_id`` values
        are never reused by uppsala's arena, so a cached wrapper stays valid for
        the life of the document; entries are weak, so the wrapper is collected
        once no Python code holds it and the slot is later swept.
        """
        if node is None:
            return None
        nid = node.node_id
        proxies = self._proxies
        r = proxies.get(nid)
        if r is not None:
            el = r()
            if el is not None:
                # Refresh the live native handle: the cached wrapper's stored
                # Node may be stale after tree mutations, but its id is unchanged.
                el._node = node
                return el
            # Dead weakref (proxy was collected): fall through and recreate.
        # Build the wrapper via the native base constructor (holder, node, id)
        # and register a callback-free weakref to it in the cache.
        el = _Element(self, node, nid)
        proxies[nid] = _weakref(el)
        # Opportunistically reclaim tombstones. Without WeakValueDictionary's
        # death callback nothing removes a dead entry on its own, so sweep here
        # once the cache crosses the armed threshold.
        if len(proxies) >= self._sweep_at:
            self._sweep()
        return el

    def _sweep(self):
        """Drop cache entries whose weakref has died, then re-arm the threshold."""
        proxies = self._proxies
        for k in [k for k, r in proxies.items() if r() is None]:
            del proxies[k]
        # Re-arm to a multiple of the surviving live count so steady-state walks
        # amortise the sweep and memory stays bounded near the live set size.
        self._sweep_at = max(256, len(proxies) * 2)

    def new_prefix(self, node):
        """Generate a fresh ``ns<N>`` prefix not already in scope on ``node``.

        Skips any ``ns<N>`` already declared on ``node`` or an ancestor so a
        generated prefix never shadows or redeclares an existing binding. A
        parsed document may itself use ``ns0``, ``ns1`` ... for its own
        namespaces, and reusing one of those would silently change the URI those
        QNames resolve to.
        """
        in_scope = set()
        n = node
        while n is not None and n.kind == "element":
            for pfx, _uri in n.namespace_declarations:
                if pfx is not None:
                    in_scope.add(pfx)
            n = n.parent
        while True:
            pfx = "ns%d" % self._ns_counter
            self._ns_counter += 1
            if pfx not in in_scope:
                return pfx


def _text_run_from(node):
    """Return adjacent text/CDATA nodes starting at ``node``.

    When ``strip_cdata=False`` is used, a single lxml ``.text`` or ``.tail``
    value can be represented by multiple adjacent native nodes, for example
    ``text`` + ``CDATA`` + ``text``.  The etree API exposes that whole run as
    one logical string.
    """
    run = []
    cur = node
    while cur is not None and cur.kind in _TEXT_KINDS:
        run.append(cur)
        cur = cur.next_sibling
    return run


def _run_text(run):
    """Convert a native text/CDATA run to the public etree string value."""
    if not run:
        return None
    return "".join(n.text or "" for n in run)


def _following_text_run(node):
    """Return the text/CDATA run immediately after ``node`` (its lxml tail)."""
    return _text_run_from(node.next_sibling)


def _remove_text_run(holder, parent, run):
    """Remove every node in a logical etree text/tail run."""
    for text_node in run:
        holder.doc.remove_child(parent, text_node)


def _attach_tail(holder, parent_node, tail_run, after):
    """Attach a detached logical tail run immediately after ``after``."""
    ref = after
    for tail_node in tail_run:
        holder.doc.insert_after(parent_node, tail_node, ref)
        ref = tail_node


def _content_children(node):
    """Return the children lxml treats as element content: elements, comments, PIs.

    Text and CDATA nodes are excluded because lxml exposes them via ``.text`` and
    ``.tail`` rather than as indexable children. Filtered natively in a single
    lock (see ``Node.content_children``); this is hot under whole-tree visits.
    """
    return node.content_children()


# ---------------------------------------------------------------------------
# Subtree cloning / moving across documents
# ---------------------------------------------------------------------------


def _clone_node(dst, snode):
    """Deep-copy ``snode`` (from another document) into ``dst`` (a ``_DocHolder``).

    Recreates the node and its whole subtree in the destination document and
    returns the new root node. Used to move elements between trees, since
    uppsala ``NodeId``s are scoped to a single document and cannot be reparented
    across documents directly.

    Delegates to the native ``Document.import_subtree``, which clones the entire
    subtree (qualified name, the element's own namespace declarations,
    attributes, text/CDATA/comment/PI and all descendants) in a single native
    pass. The previous pure-Python recursion made one FFI call per node and was
    the top cost of pyFF's aggregation step once traversal went native (see
    pyFF/performance.md); doing the whole copy natively removes that per-node
    Python+FFI overhead. Namespaces inherited from ancestors outside the moved
    subtree are added separately by :func:`_copy_inherited_ns`.
    """
    return dst.doc.import_subtree(snode)


def _copy_inherited_ns(dst, snode, dnode):
    """Declare on ``dnode`` the namespaces ``snode`` inherits from its ancestors.

    ``_clone_node`` copies only each element's own ``xmlns`` declarations, so a
    subtree that relied on a prefix (or default namespace) declared on an
    ancestor *outside* the moved subtree would lose that binding after a
    cross-tree move and serialize incorrectly. Walk ``snode``'s ancestors and
    redeclare every in-scope binding it does not already declare itself onto the
    cloned root ``dnode``, so all prefixes the subtree uses stay in scope. A
    descendant that is genuinely in no namespace carries its own ``xmlns=""``
    reset (preserved by ``_clone_node``), so this never recaptures it.
    """
    own = {pfx for pfx, _uri in snode.namespace_declarations}
    seen = set(own)
    n = snode.parent
    while n is not None and n.kind == "element":
        for pfx, uri in n.namespace_declarations:
            if pfx not in seen:
                seen.add(pfx)
                dst.doc.set_namespace_declaration(dnode, pfx, uri)
        n = n.parent


def _repoint_subtree(src_holder, dst, snode, dnode):
    """Move any live proxies from the source subtree onto the cloned subtree.

    Walks the source (``snode``) and cloned (``dnode``) trees in lock-step. When
    a source node has a live wrapper, that wrapper is re-pointed at the cloned
    node and re-registered in the destination cache, so existing Python
    references to a moved element (and its descendants) remain valid afterward -
    mirroring lxml's in-place move semantics.
    """
    # Fast path: once the source holder has no live proxies left, there is
    # nothing in the (possibly large) subtree to repoint, so skip the walk
    # entirely. This is the common case for pyFF's ``deepcopy(entity)`` +
    # ``append`` aggregation: the freshly deep-copied source holds exactly one
    # live proxy (its root), so after repointing it the whole descendant walk
    # (hundreds of nodes per entity) is avoided.
    #
    # The weakrefs are callback-free (no cache eviction on collection), so the
    # dict can hold dead tombstones and stay non-empty after every proxy has
    # been garbage-collected. Sweep them first so a subtree with only dead
    # entries still takes the fast path instead of walking the whole thing.
    proxies = src_holder._proxies
    if proxies:
        dead = [nid for nid, ref in proxies.items() if ref() is None]
        for nid in dead:
            proxies.pop(nid, None)
    if not proxies:
        return
    r = src_holder._proxies.get(snode.node_id)
    proxy = r() if r is not None else None
    if r is not None:
        # Drop the source entry (a live proxy is moved below; a dead tombstone
        # is simply reclaimed here).
        src_holder._proxies.pop(snode.node_id, None)
    if proxy is not None:
        proxy._holder = dst
        proxy._node = dnode
        proxy._id = dnode.node_id
        dst._proxies[dnode.node_id] = _weakref(proxy)
        if not src_holder._proxies:
            return
    # Children are cloned in the same order, so a positional zip pairs them up.
    for sc, dc in zip(snode.children, dnode.children):
        _repoint_subtree(src_holder, dst, sc, dc)


def _extract(holder, node):
    """Detach ``node`` and its logical tail run from their parent.

    Returns the detached tail run. The tail travels with the element so that
    moving or removing the element also moves/removes its trailing text,
    matching lxml/ElementTree semantics even when CDATA preservation split the
    tail across adjacent text/CDATA nodes.
    """
    tail = _following_text_run(node)
    if node.parent is not None:
        # Detach the tail first while it is still a sibling, then the node.
        for text_node in tail:
            holder.doc.detach(text_node)
        holder.doc.detach(node)
    return tail


def _attach(holder, parent_node, node, tail, ref=None):
    """Insert ``node`` and then reattach its logical tail run after it."""
    if ref is None:
        holder.doc.append_child(parent_node, node)
    else:
        holder.doc.insert_before(parent_node, node, ref)
    _attach_tail(holder, parent_node, tail, node)


# ---------------------------------------------------------------------------
# Element construction
# ---------------------------------------------------------------------------


def _build_element(holder, tag, nsmap):
    nsmap = _validate_nsmap(nsmap)
    ns, local = _split_key(tag)
    prefix = _prefix_for_ns(ns, nsmap) if ns else None
    node = holder.doc.create_element(local, ns, prefix)
    if nsmap:
        for pfx, uri in nsmap.items():
            holder.doc.set_namespace_declaration(node, pfx, uri)
    return node


def _finalize_element_ns(holder, node):
    """Ensure an attached element's namespace is serializable, reusing an
    in-scope prefix/default declaration when one exists rather than emitting a
    redundant ``xmlns`` declaration."""
    q = node.tag
    ns = q.namespace_uri
    if not ns:
        return
    # Walk self + ancestors looking for an in-scope declaration of ``ns``.
    n = node
    while n is not None and n.kind == "element":
        for pfx, uri in n.namespace_declarations:
            if uri == ns:
                if pfx is None:
                    if q.prefix is None:
                        return  # inherits the default namespace
                else:
                    if q.prefix != pfx:
                        node.set_qname(q.local_name, ns, pfx)
                    return
        n = n.parent
    # Not in scope anywhere: declare it on this element.
    prefix = q.prefix or _namespace_map.get(ns)
    if prefix is None:
        prefix = holder.new_prefix(node)
    if q.prefix != prefix:
        node.set_qname(q.local_name, ns, prefix)
    holder.doc.set_namespace_declaration(node, prefix, ns)


def _apply_attribs(el, attrib, extra):
    """Apply an ``attrib`` dict and ``**extra`` keyword attributes to ``el``."""
    if attrib:
        for k, v in attrib.items():
            el.set(k, v)
    for k, v in extra.items():
        # Keyword attributes are passed through as attribute names verbatim,
        # matching lxml's Element(tag, key="value") shorthand.
        el.set(k, v)


def Element(_tag, attrib=None, nsmap=None, **extra):
    """Create a new standalone element (the root of its own tree)."""
    holder = _DocHolder(_u.Document.empty())
    node = _build_element(holder, _tag, nsmap)
    holder.doc.append_child(holder.doc.root, node)
    _finalize_element_ns(holder, node)
    el = holder.proxy(node)
    _apply_attribs(el, attrib, extra)
    return el


def SubElement(_parent, _tag, attrib=None, nsmap=None, **extra):
    """Create a child element of ``_parent`` in the same document."""
    holder = _parent._holder
    node = _build_element(holder, _tag, nsmap)
    holder.doc.append_child(_parent._node, node)
    _finalize_element_ns(holder, node)
    el = holder.proxy(node)
    _apply_attribs(el, attrib, extra)
    return el


def Comment(text=None):
    """Create a standalone comment node (its ``.tag`` is the ``Comment`` factory)."""
    holder = _DocHolder(_u.Document.empty())
    node = holder.doc.create_comment("" if text is None else text)
    holder.doc.append_child(holder.doc.root, node)
    return holder.proxy(node)


def ProcessingInstruction(target, text=None):
    """Create a standalone processing-instruction node."""
    holder = _DocHolder(_u.Document.empty())
    node = holder.doc.create_processing_instruction(target, text)
    holder.doc.append_child(holder.doc.root, node)
    return holder.proxy(node)


PI = ProcessingInstruction


def _set_element_tag(el, value):
    """Rename ``el`` (the cold ``_Element.tag`` setter), keeping its namespace
    declared/in scope.

    Invoked by the native ``_ElementBase.tag`` setter, which keeps the hot getter
    in Rust but forwards the intricate namespace-finalisation here rather than
    re-implementing it natively.
    """
    ns, local = _split_key(value)
    prefix = _prefix_for_ns(ns, el.nsmap) if ns else None
    el._node.set_qname(local, ns, prefix)
    if ns:
        # Reuse an in-scope binding for ``ns`` when one exists - including an
        # inherited default namespace - and only declare or generate a prefix
        # when none is in scope, rather than always forcing a prefix.
        _finalize_element_ns(el._holder, el._node)


def _set_element_text(el, value):
    """Set ``el``'s leading text (cold ``_Element.text`` setter), replacing the
    full existing text/CDATA run. For comment/PI nodes sets the body instead."""
    node = el._node
    kind = node.kind
    if kind == "comment":
        node.set_text("" if value is None else value)
        return
    if kind == "processing_instruction":
        node.set_pi_data(value)
        return
    doc = el._holder.doc
    fc = node.first_child
    run = _text_run_from(fc)
    if value is None:
        _remove_text_run(el._holder, node, run)
        return
    if run:
        # Reuse the first native text-like node, then remove the rest so the
        # public single-string assignment replaces the whole logical run.
        run[0].set_text(value)
        _remove_text_run(el._holder, node, run[1:])
    else:
        # No leading text node yet: create one and make it the first child.
        tn = doc.create_text(value)
        if fc is None:
            doc.append_child(node, tn)
        else:
            doc.insert_before(node, tn, fc)


def _set_element_tail(el, value):
    """Set ``el``'s trailing text (cold ``_Element.tail`` setter), replacing the
    full existing text/CDATA run."""
    node = el._node
    doc = el._holder.doc
    parent = node.parent
    if value is None:
        if parent is not None:
            _remove_text_run(el._holder, parent, _following_text_run(node))
        return
    run = _following_text_run(node)
    if run:
        run[0].set_text(value)
        if parent is not None:
            _remove_text_run(el._holder, parent, run[1:])
    elif parent is not None:
        # A tail can only exist where there is a parent to host the text node.
        tn = doc.create_text(value)
        doc.insert_after(parent, tn, node)


# Hand the native ElementBase the etree-layer callables it needs: the
# Comment/ProcessingInstruction tag sentinels (returned by the native .tag
# getter for comment/PI nodes) and the cold property-setter helpers above.
_u._register_element_helpers(
    Comment,
    ProcessingInstruction,
    _set_element_tag,
    _set_element_text,
    _set_element_tail,
)


# ---------------------------------------------------------------------------
# _Element
# ---------------------------------------------------------------------------


class _Element(_u._ElementBase):
    """A live view over a node in a native Document. Compatible with lxml's
    ``_Element``.

    Subclasses the native ``_ElementBase``, which owns the ``(_holder, _node,
    _id)`` state (exposed as read/write properties) and the hot methods that
    have been ported to Rust; the methods below are the remaining Python layer.
    ``__slots__ = ()`` keeps instances dict-free -- all state lives in the native
    base, and ``__weakref__`` is provided by the base's ``weakref`` support so
    the per-document proxy cache can hold callback-free weakrefs.
    """

    __slots__ = ()

    # -- identity / repr --------------------------------------------------

    def __repr__(self):
        """An lxml-style repr, distinguishing elements, comments and PIs."""
        kind = self._node.kind
        if kind == "comment":
            return "<!--%s-->" % (self._node.comment_text or "")
        if kind == "processing_instruction":
            return "<?%s?>" % self._node.pi_target
        return "<Element %s at 0x%x>" % (self.tag, id(self))

    # -- tag --------------------------------------------------------------
    # The `tag` property (Clark-notation getter + rename setter) is provided
    # natively by _ElementBase. The getter returns the Clark string directly for
    # elements (the hot path) and the Comment/ProcessingInstruction factory for
    # comment/PI nodes; the setter delegates the cold namespace-finalisation back
    # to `_set_element_tag` below (registered via _register_element_helpers).

    # -- text / tail ------------------------------------------------------
    # ElementTree's .text/.tail are virtual views over real sibling text nodes
    # in uppsala's DOM: .text is the element's leading text-node child, and
    # .tail is the text node immediately following the element. Storing them as
    # real nodes means serialization needs no special handling.
    #
    # Both properties are provided natively by _ElementBase: the getters collect
    # the leading/trailing Text+CDATA run under one lock (and return the
    # comment/PI body for those node kinds), while the cold, mutation-heavy
    # setters delegate to `_set_element_text` / `_set_element_tail` below
    # (registered via _register_element_helpers).

    # -- attributes -------------------------------------------------------

    @property
    def attrib(self):
        """A live ``dict``-like view of this element's attributes."""
        return _Attrib(self)

    def _ensure_ns_prefix(self, ns):
        """Return a serialization prefix for ``ns`` that is in scope on this
        element, declaring one here if none is inherited.

        Walks self and ancestors for an existing non-default declaration of
        ``ns``; otherwise falls back to a registered prefix or a generated
        ``ns<N>`` and declares it on this element.
        """
        n = self._node
        while n is not None and n.kind == "element":
            for pfx, uri in n.namespace_declarations:
                if uri == ns and pfx is not None:
                    return pfx
            n = n.parent
        pfx = _namespace_map.get(ns)
        if pfx is None:
            pfx = self._holder.new_prefix(self._node)
        self._holder.doc.set_namespace_declaration(self._node, pfx, ns)
        return pfx

    def _attr_clark(self, a):
        """Return an attribute's name in Clark ``{uri}local`` notation."""
        n = a.name
        return _make_clark(n.namespace_uri, n.local_name)

    def get(self, key, default=None):
        """Return the attribute value for ``key`` (str or QName), or ``default``.

        A plain key matches the attribute with *no* namespace; a Clark
        ``{uri}local`` key (or QName) matches that exact namespace. An attribute
        in a different namespace that shares the local name is not returned.
        """
        ns, local = _split_key(key)
        if ns is None:
            # Plain key: match the no-namespace attribute exactly, rather than
            # the first attribute with this local name in any namespace.
            for a in self._node.attributes:
                an = a.name
                if an.local_name == local and an.namespace_uri is None:
                    return a.value
            return default
        v = self._node.get_attribute(local, ns)
        return v if v is not None else default

    def set(self, key, value):
        """Set attribute ``key`` (str or QName) to ``value``.

        For namespaced attributes, ensures a usable prefix is in scope (declaring
        one if needed) so the attribute serializes correctly.
        """
        ns, local = _split_key(key)
        prefix = None
        if ns:
            prefix = self._ensure_ns_prefix(ns)
        self._node.set_attribute(local, value, ns, prefix)

    def keys(self):
        """Return the attribute names (Clark notation) in document order."""
        return [self._attr_clark(a) for a in self._node.attributes]

    def values(self):
        """Return the attribute values in document order."""
        return [a.value for a in self._node.attributes]

    def items(self):
        """Return ``(name, value)`` attribute pairs in document order."""
        return [(self._attr_clark(a), a.value) for a in self._node.attributes]

    # -- children / sequence protocol ------------------------------------
    # Only element/comment/PI children participate; text/CDATA are surfaced via
    # .text/.tail instead (see _content_children).

    # __len__ is provided natively by _ElementBase (counts content children
    # without materialising the child list).

    def __iter__(self):
        """Iterate over child elements (and comments/PIs) as proxies."""
        proxy = self._holder.proxy
        return (proxy(k) for k in _content_children(self._node))

    def __getitem__(self, index):
        """Index or slice into the child elements."""
        kids = _content_children(self._node)
        proxy = self._holder.proxy
        if isinstance(index, slice):
            return [proxy(k) for k in kids[index]]
        return proxy(kids[index])

    def __setitem__(self, index, element):
        """Replace the child at ``index`` with ``element``."""
        if isinstance(index, slice):
            raise NotImplementedError("slice assignment is not supported in v1")
        kids = _content_children(self._node)
        old = kids[index]
        # Insert the new child in place, then remove the old one (with its tail).
        node, tail = self._adopt(element)
        self._holder.doc.insert_before(self._node, node, old)
        _extract(self._holder, old)
        _attach_tail(self._holder, self._node, tail, node)

    def __delitem__(self, index):
        """Remove the child at ``index`` (or the children in a slice)."""
        kids = _content_children(self._node)
        targets = kids[index] if isinstance(index, slice) else [kids[index]]
        for k in targets:
            _extract(self._holder, k)

    def _is_child(self, element):
        """True if ``element`` is a direct child of this element.

        Requires the same backing document: ``node_id`` values are scoped per
        document, so a node from another tree can share an id with one of ours
        and must not be mistaken for a child.
        """
        if not isinstance(element, _Element) or element._holder is not self._holder:
            return False
        p = element._node.parent
        return p is not None and p.node_id == self._id

    def __contains__(self, element):
        """True if ``element`` is a direct child of this element."""
        return self._is_child(element)

    def index(self, child, start=None, stop=None):
        """Return the position of ``child`` among this element's children.

        ``start``/``stop`` bound the search range and follow list/lxml
        semantics: a negative value counts from the end of the child list.
        Raises ValueError if ``child`` is not a child of this element (including
        a non-element or an element from a different document) or falls outside
        the requested range.
        """
        if not isinstance(child, _Element) or child._holder is not self._holder:
            raise ValueError("element is not a child of this node")
        kids = _content_children(self._node)
        ids = [k.node_id for k in kids]
        try:
            pos = ids.index(child._node.node_id)
        except ValueError:
            raise ValueError("element is not a child of this node") from None
        # Normalize negative bounds to offsets from the end, like list.index.
        n = len(kids)
        if start is not None:
            if start < 0:
                start = max(n + start, 0)
            if pos < start:
                raise ValueError("element is not in the requested range")
        if stop is not None:
            if stop < 0:
                stop += n
            if pos >= stop:
                raise ValueError("element is not in the requested range")
        return pos

    # -- mutation ---------------------------------------------------------

    def _adopt(self, element):
        """Return ``(native_node, tail)`` in this holder for ``element``,
        cloning across documents when necessary."""
        if not isinstance(element, _Element):
            # All mutation entry points (append/insert/replace/addnext/...) flow
            # through here, so this is the single place to reject non-elements
            # with lxml's TypeError instead of a stray AttributeError.
            raise TypeError(
                "Argument must be an Element, got %s" % type(element).__name__
            )
        if element._holder is self._holder:
            tail = _extract(self._holder, element._node)
            return element._node, tail
        return self._clone_into_self(element)

    def _clone_into_self(self, src_el):
        """Deep-copy ``src_el`` (from another document) into this holder.

        Returns ``(new_node, new_tail)``. The source subtree is cloned, its live
        proxies are re-pointed onto the clones, and the original is detached from
        its tree, so the net effect is a move with preserved object identity.
        """
        sh = src_el._holder
        dst = self._holder
        snode = src_el._node
        stail = _following_text_run(snode)
        new_node = _clone_node(dst, snode)
        # Carry over namespaces inherited from ancestors outside the moved
        # subtree so prefixed/defaulted names stay declared in the destination.
        _copy_inherited_ns(dst, snode, new_node)
        new_tail = [_clone_node(dst, text_node) for text_node in stail]
        _repoint_subtree(sh, dst, snode, new_node)
        if snode.parent is not None:
            for text_node in stail:
                sh.doc.detach(text_node)
            sh.doc.detach(snode)
        return new_node, new_tail

    def append(self, element):
        """Append ``element`` as the last child (moving it if it has a parent)."""
        node, tail = self._adopt(element)
        _attach(self._holder, self._node, node, tail)

    def extend(self, elements):
        """Append each element in ``elements`` in order."""
        for el in elements:
            self.append(el)

    def insert(self, index, element):
        """Insert ``element`` at ``index`` among the child elements."""
        node, tail = self._adopt(element)
        kids = _content_children(self._node)
        n = len(kids)
        # Clamp the index like list.insert: negative counts from the end, and an
        # out-of-range index appends.
        if index < 0:
            index += n
        if index < 0:
            index = 0
        ref = kids[index] if index < n else None
        _attach(self._holder, self._node, node, tail, ref)

    def remove(self, element):
        """Remove direct child ``element`` (and its tail). Raises if not a child."""
        if not self._is_child(element):
            raise ValueError("Element is not a child of this node.")
        _extract(self._holder, element._node)

    def replace(self, old_element, new_element):
        """Replace child ``old_element`` with ``new_element`` in place."""
        if not self._is_child(old_element):
            raise ValueError("Element is not a child of this node.")
        node, tail = self._adopt(new_element)
        self._holder.doc.insert_before(self._node, node, old_element._node)
        _extract(self._holder, old_element._node)
        _attach_tail(self._holder, self._node, tail, node)

    def addnext(self, element):
        """Insert ``element`` as this element's next sibling."""
        parent = self._node.parent
        if parent is None:
            raise TypeError("cannot add sibling to a root element")
        node, tail = self._adopt(element)
        self._holder.doc.insert_after(parent, node, self._node)
        _attach_tail(self._holder, parent, tail, node)

    def addprevious(self, element):
        """Insert ``element`` as this element's previous sibling."""
        parent = self._node.parent
        if parent is None:
            raise TypeError("cannot add sibling to a root element")
        node, tail = self._adopt(element)
        self._holder.doc.insert_before(parent, node, self._node)
        _attach_tail(self._holder, parent, tail, node)

    def makeelement(self, _tag, attrib=None, nsmap=None, **extra):
        """Create a new (detached) element in the same document as this one."""
        node = _build_element(self._holder, _tag, nsmap)
        _finalize_element_ns(self._holder, node)
        el = self._holder.proxy(node)
        _apply_attribs(el, attrib, extra)
        return el

    def __copy__(self):
        """Return a detached deep copy in its own document (lxml copies subtrees)."""
        return self.__deepcopy__(None)

    def __deepcopy__(self, memo):
        """Return a detached deep copy of this element and its subtree.

        Serializing (with inherited namespace declarations) and reparsing yields
        an independent element in a fresh document, matching lxml, where
        ``copy.deepcopy(element)`` produces a standalone subtree. The copy carries
        no parent and no tail.
        """
        return fromstring(tostring(self, encoding="unicode"))

    # -- navigation -------------------------------------------------------

    def getparent(self):
        """Return the parent element, or None at the tree root."""
        p = self._node.parent
        if p is None or p.kind == "document":
            return None
        return self._holder.proxy(p)

    def getnext(self):
        """Return the next sibling element (skipping text nodes), or None."""
        n = self._node.next_sibling
        while n is not None and n.kind in _TEXT_KINDS:
            n = n.next_sibling
        return self._holder.proxy(n) if n is not None else None

    def getprevious(self):
        """Return the previous sibling element (skipping text nodes), or None."""
        n = self._node.previous_sibling
        while n is not None and n.kind in _TEXT_KINDS:
            n = n.previous_sibling
        return self._holder.proxy(n) if n is not None else None

    def itersiblings(self, tag=None, preceding=False):
        """Yield following (or, with ``preceding=True``, preceding) sibling
        elements, optionally filtered by ``tag``."""
        sib = self.getprevious if preceding else self.getnext
        cur = sib()
        while cur is not None:
            if tag is None or _tag_matches(cur._node, tag):
                yield cur
            cur = cur.getprevious() if preceding else cur.getnext()

    def iterancestors(self, tag=None):
        """Yield ancestor elements from parent upward, optionally filtered by ``tag``."""
        cur = self.getparent()
        while cur is not None:
            if tag is None or _tag_matches(cur._node, tag):
                yield cur
            cur = cur.getparent()

    def iterdescendants(self, tag=None):
        """Yield all descendant elements (excluding self) in document order."""
        for e in self.iter(tag):
            if e is not self:
                yield e

    def getroottree(self):
        """Return an :class:`_ElementTree` wrapping this element's document."""
        return _ElementTree(self._holder, self._holder.doc.document_element)

    # -- traversal --------------------------------------------------------

    def iter(self, tag=None):
        """Iterate this element and all descendants in document (pre-order) order.

        Matching follows lxml/ElementTree:

        * ``tag=None`` yields everything (elements, comments and PIs);
        * ``tag="*"`` is an element-only wildcard (comments/PIs excluded);
        * a specific tag yields only matching elements.

        Implementation note: this is the hottest path in the layer (callers and
        ElementPath walk large subtrees repeatedly). The pre-order tree walk and
        tag matching run **natively** in ``Node.iter_descendants`` (one mutex
        acquisition per step, not per visited node, with no per-node Python
        attribute access or Clark-string building), so the only Python-level
        cost here is wrapping the nodes that actually match in their identity
        proxies. For a find-first pattern (``next(el.iter(tag))``) the native
        iterator stops as soon as the first match is found.
        """
        proxy = self._holder.proxy
        # Normalise a QName argument to its Clark-notation string; the native
        # filter understands None, "*", "{ns}local" and bare local names.
        if isinstance(tag, QName):
            tag = tag.text
        for node in self._node.iter_descendants(tag):
            yield proxy(node)

    def itertext(self):
        """Yield all text and tail content in this subtree, in document order."""
        # Ported from xml.etree.ElementTree.itertext: comments/PIs (non-str tags)
        # contribute no text.
        tag = self.tag
        if not isinstance(tag, str) and tag is not None:
            return
        t = self.text
        if t:
            yield t
        for e in self:
            yield from e.itertext()
            if e.tail:
                yield e.tail

    # -- search -----------------------------------------------------------

    def find(self, path, namespaces=None):
        """Return the first subelement matching the ElementPath ``path``, or None."""
        return ElementPath.find(self, path, namespaces)

    def findall(self, path, namespaces=None):
        """Return all subelements matching the ElementPath ``path`` as a list."""
        return ElementPath.findall(self, path, namespaces)

    def findtext(self, path, default=None, namespaces=None):
        """Return the text of the first match of ``path``, or ``default``."""
        return ElementPath.findtext(self, path, default, namespaces)

    def iterfind(self, path, namespaces=None):
        """Iterate over all subelements matching the ElementPath ``path``."""
        return ElementPath.iterfind(self, path, namespaces)

    def xpath(self, _path, namespaces=None, smart_strings=True, **variables):
        """Evaluate a full XPath 1.0 expression against this element as context.

        Delegates to the native :class:`pyuppsala.XPathEvaluator`. Node-set
        results are returned as ``_Element`` proxies; string/number/boolean
        results are returned as the corresponding Python types.

        ``smart_strings`` is accepted for lxml call-signature compatibility and
        ignored: pyuppsala already returns plain ``str`` (never lxml's
        parent-aware "smart" strings), so the flag has no effect.

        XPath variable binding (lxml's ``$name`` keyword arguments) is not
        supported by the underlying engine; passing any raises
        ``NotImplementedError`` rather than silently ignoring it.
        """
        del smart_strings  # accepted for lxml compatibility; no behavioral effect
        if variables:
            raise NotImplementedError(
                "XPath variable binding is not supported (got %r)"
                % sorted(variables)
            )
        # lxml's .xpath() has no per-evaluation node-visit cap; the native
        # evaluator defaults to one (anti-DoS) that is far too low for large
        # trusted documents (e.g. a SAML aggregate with thousands of entities).
        # The budget is the module-level ``MAX_XPATH_NODE_VISITS`` (unbounded by
        # default to match lxml), which applications can lower for untrusted input.
        ev = _u.XPathEvaluator(max_node_visits=MAX_XPATH_NODE_VISITS)
        if namespaces:
            for pfx, uri in namespaces.items():
                if pfx:
                    ev.add_namespace(pfx, uri)
        # Build the attribute-node and document-order indexes the engine needs
        # for the attribute axis (``@name``) and correct node-set ordering/dedup.
        # prepare_xpath() rebuilds from scratch, so it also picks up any tree
        # mutations made since the last evaluation.
        self._holder.doc.prepare_xpath()
        try:
            result = ev.evaluate(self._holder.doc, _path, self._node)
        except _u.XPathError as e:
            raise XPathEvalError(str(e)) from e
        return _wrap_xpath_result(self._holder, result)

    def xinclude(self, *, network_access=False):
        """Process W3C XInclude ``xi:include`` directives in this subtree.

        Replaces each ``{http://www.w3.org/2001/XInclude}include`` element with
        the referenced resource: ``parse="xml"`` (the default) splices in the
        included document's root element; ``parse="text"`` inserts its text. A
        child ``xi:fallback`` provides content if the resource cannot be loaded.
        Relative ``href`` values resolve against the document's ``base_url``
        (set by :func:`parse`); ``file`` URLs and filesystem paths are always
        supported. Processing is recursive (included XML is itself scanned).

        Remote ``http(s)``/``ftp`` targets are only fetched when
        ``network_access=True``. The default is off (matching lxml parsers'
        ``no_network=True`` default) so that running XInclude over untrusted XML
        cannot be turned into an SSRF / data-exfiltration vector; a blocked
        network target behaves like any other load failure (its ``xi:fallback``
        is used if present, otherwise :class:`XIncludeError` is raised).
        """
        _process_xincludes(
            self, self._holder.base_url, allow_network=network_access
        )

    # -- misc properties --------------------------------------------------

    # nsmap / prefix / sourceline are provided natively by _ElementBase:
    #   * nsmap   -- in-scope prefix->URI dict, built in Rust from the native
    #                ancestor walk (inner binding wins, None key = default ns);
    #   * prefix  -- the element tag's namespace prefix, or None;
    #   * sourceline -- the 1-based source line, or None for built nodes.

    @property
    def base(self):
        """The xml:base URI of this element. Not tracked in v1 (always None)."""
        return None


def _tag_matches(node, tag):
    """True if ``node`` is an element whose Clark-notation tag equals ``tag``."""
    if node.kind != "element":
        return False
    q = node.tag
    if q is None:
        return False
    # Compare (namespace, local) components directly rather than building a
    # "{ns}local" string for every call; equivalent to the old
    # ``_make_clark(q.namespace_uri, q.local_name) == tag`` but allocation-free.
    if isinstance(tag, QName):
        tag = tag.text
    if tag and tag[0] == "{":
        ns, _, local = tag[1:].partition("}")
    else:
        ns, local = "", tag
    return q.local_name == local and (q.namespace_uri or "") == ns


def _wrap_xpath_result(holder, result):
    """Convert a native XPath result to lxml-compatible Python values.

    For node-set results, element/comment/PI nodes become ``_Element``
    proxies while text and CDATA nodes (e.g. from a ``text()`` selection)
    become plain ``str`` values, matching lxml which returns string results
    for text-node selections rather than node objects. Scalar results
    (bool/float/str) pass through unchanged.
    """
    if isinstance(result, list):
        wrapped = []
        for n in result:
            if isinstance(n, _u.Node):
                if n.kind in _TEXT_KINDS:
                    wrapped.append(n.text or "")
                elif n.kind == "attribute":
                    # Attribute-axis results (``@name``) become their string
                    # value, matching lxml's ``xpath("...//@attr")``.
                    wrapped.append(n.attribute_value or "")
                else:
                    wrapped.append(holder.proxy(n))
            else:
                wrapped.append(n)
        return wrapped
    return result


# ---------------------------------------------------------------------------
# Attribute mapping view
# ---------------------------------------------------------------------------


class _Attrib:
    """A live ``dict``-like view over an element's attributes (lxml's ``.attrib``).

    Keys are attribute names in Clark ``{uri}local`` notation (or plain names);
    all reads and writes go straight to the backing element.
    """

    __slots__ = ("_el",)

    def __init__(self, el):
        self._el = el

    def __getitem__(self, key):
        v = self._el.get(key)
        if v is None:
            raise KeyError(key)
        return v

    def __setitem__(self, key, value):
        self._el.set(key, value)

    def __delitem__(self, key):
        ns, local = _split_key(key)
        # Pass the namespace so a namespaced attribute is matched exactly, not
        # by local name alone (which could delete a same-named attribute in a
        # different namespace).
        old = self._el._node.remove_attribute(local, ns)
        if old is None:
            raise KeyError(key)

    def __contains__(self, key):
        return self._el.get(key) is not None

    def __len__(self):
        return len(self._el._node.attributes)

    def __iter__(self):
        return iter(self._el.keys())

    def get(self, key, default=None):
        """Return the value for ``key``, or ``default`` if absent."""
        return self._el.get(key, default)

    def keys(self):
        """Attribute names in document order."""
        return self._el.keys()

    def values(self):
        """Attribute values in document order."""
        return self._el.values()

    def items(self):
        """``(name, value)`` pairs in document order."""
        return self._el.items()

    def update(self, other):
        """Set multiple attributes from a mapping or iterable of pairs."""
        pairs = other.items() if hasattr(other, "items") else other
        for k, v in pairs:
            self._el.set(k, v)

    def __repr__(self):
        return repr(dict(self._el.items()))


def iselement(element):
    """Return True if ``element`` is a pyuppsala.etree element."""
    return isinstance(element, _Element)


# ---------------------------------------------------------------------------
# ElementTree
# ---------------------------------------------------------------------------


class DocInfo:
    """Document-level metadata for a tree (lxml's ``DocInfo``).

    Currently exposes the preserved ``<!DOCTYPE ...>`` declaration. Uppsala
    keeps the DOCTYPE verbatim for round-trip fidelity but does not process it,
    so the richer lxml fields derived from a parsed DTD (``public_id``,
    ``system_url``, ``internalDTD``, ...) are not available.
    """

    __slots__ = ("_holder",)

    def __init__(self, holder):
        """Wrap the ``_DocHolder`` whose native document carries the metadata."""
        self._holder = holder

    @property
    def doctype(self):
        """The raw ``<!DOCTYPE ...>`` string, or ``""`` when the document has none.

        Matches lxml, which returns an empty string (not ``None``) for a
        document without a document type declaration.
        """
        dt = self._holder.doc.doctype
        return dt if dt is not None else ""


class _ElementTree:
    """A document wrapper (lxml's ``_ElementTree``) holding a root element."""

    def __init__(self, holder, root=None):
        """Wrap ``holder`` with an optional selected tree root.

        ``ElementTree(child)`` in lxml serializes and validates from ``child``,
        not from the document's outer element.  Keeping the selected native root
        here prevents callers from accidentally exposing sibling subtrees when
        they meant to operate on a nested element.
        """
        self._holder = holder
        self._root = root

    def getroot(self):
        """Return the root element, or None for an empty document."""
        root = self._root
        return self._holder.proxy(root) if root is not None else None

    @property
    def docinfo(self):
        """Return a :class:`DocInfo` exposing this tree's document metadata."""
        return DocInfo(self._holder)

    def _require_root(self):
        root = self.getroot()
        if root is None:
            raise AssertionError("ElementTree not initialized, missing root")
        return root

    def parse(self, source, parser=None, base_url=None):
        """Parse ``source`` into this tree, replacing its contents. Returns the root."""
        data = _read_source(source)
        el = fromstring(data, parser)
        if base_url is None and isinstance(source, (str, bytes, os.PathLike)):
            base_url = os.fspath(source)
            if isinstance(base_url, bytes):
                base_url = base_url.decode("utf-8", "replace")
        el._holder.base_url = base_url
        self._holder = el._holder
        self._root = el._node
        return el

    def xinclude(self, *, network_access=False):
        """Process W3C XInclude ``xi:include`` directives in place.

        Relative ``href`` references resolve against the tree's ``base_url``
        (set by :func:`parse`). See :meth:`_Element.xinclude` (including the
        ``network_access`` opt-in for remote targets).
        """
        root = self.getroot()
        if root is not None:
            root.xinclude(network_access=network_access)

    def write(
        self,
        file,
        encoding=None,
        xml_declaration=None,
        pretty_print=False,
        **kwargs,
    ):
        """Serialize the tree to ``file`` (a path or writable file object)."""
        data = tostring(
            self._require_root(),
            encoding=encoding,
            xml_declaration=xml_declaration,
            pretty_print=pretty_print,
            **kwargs,
        )
        if isinstance(file, (str, bytes, os.PathLike)):
            mode = "w" if isinstance(data, str) else "wb"
            if isinstance(data, str):
                with open(file, mode, encoding="utf-8") as fh:
                    fh.write(data)
            else:
                with open(file, mode) as fh:
                    fh.write(data)
        else:
            file.write(data)

    def find(self, path, namespaces=None):
        """Find the first matching subelement from the root."""
        return self._require_root().find(path, namespaces)

    def findall(self, path, namespaces=None):
        """Find all matching subelements from the root."""
        return self._require_root().findall(path, namespaces)

    def findtext(self, path, default=None, namespaces=None):
        """Find the text of the first matching subelement from the root."""
        return self._require_root().findtext(path, default, namespaces)

    def iterfind(self, path, namespaces=None):
        """Iterate matching subelements from the root."""
        return self._require_root().iterfind(path, namespaces)

    def iter(self, tag=None):
        """Iterate the root element and all its descendants."""
        root = self.getroot()
        if root is None:
            return iter(())
        return root.iter(tag)

    def xpath(self, path, namespaces=None, smart_strings=True, **variables):
        """Evaluate an XPath expression with the root as context.

        ``smart_strings`` is accepted for lxml compatibility and ignored.
        """
        return self._require_root().xpath(
            path, namespaces=namespaces, smart_strings=smart_strings, **variables
        )

    def getpath(self, element):
        """Return an absolute XPath locating ``element`` within this tree.

        Positional predicates (``tag[n]``) are added only where a tag is
        ambiguous among its siblings, matching lxml's ``getpath``.  lxml keeps
        the selected root itself document-absolute even for
        ``ElementTree(child)``, but reports descendants relative to that
        selected root.
        """
        if not isinstance(element, _Element) or element._holder is not self._holder:
            raise ValueError("Element is not in this tree")
        selected = self._require_root()
        stop_at_selected = element._id != selected._id
        if stop_at_selected:
            cur = element.getparent()
            while cur is not None and cur._id != selected._id:
                cur = cur.getparent()
            stop_at_selected = cur is not None
        parts = []
        cur = element
        while cur is not None:
            parent = cur.getparent()
            if stop_at_selected and cur._id == selected._id:
                parts.append("/" + _path_tag(cur))
                break
            if parent is None:
                parts.append("/" + _path_tag(cur))
                break
            # Only number this step if its tag is not unique among siblings.
            same = [c for c in parent if c.tag == cur.tag]
            tag = _path_tag(cur)
            if len(same) > 1:
                pos = same.index(cur) + 1
                parts.append("/%s[%d]" % (tag, pos))
            else:
                parts.append("/" + tag)
            cur = parent
        return "".join(reversed(parts))


def _path_tag(element):
    """Return an element's tag for use in an XPath step (``prefix:local``)."""
    q = element._node.tag
    if q is None:
        return "*"
    if q.prefix:
        return "%s:%s" % (q.prefix, q.local_name)
    return q.local_name


def ElementTree(element=None, *, file=None, parser=None):
    """Create an :class:`_ElementTree`, lxml's document wrapper.

    Wraps ``element``'s document, parses ``file``, or (with neither) creates an
    empty tree. Exposed as a factory function rather than a class.
    """
    if element is not None:
        return _ElementTree(element._holder, element._node)
    if file is not None:
        return parse(file, parser)
    holder = _DocHolder(_u.Document.empty())
    return _ElementTree(holder)


# ---------------------------------------------------------------------------
# Parsing & serialization
# ---------------------------------------------------------------------------


_HUGE_DEPTH = 1 << 30
# The native parser takes max_entity_expansion as a platform-sized ``usize``.
# Clamp to ``sys.maxsize`` so ``huge_tree`` does not overflow on 32-bit builds
# (where ``1 << 40`` exceeds ``usize``); this is still effectively unbounded.
_HUGE_ENTITY = min(1 << 40, sys.maxsize)


class XMLParser:
    """A configurable parser, mapping lxml options onto uppsala's knobs."""

    def __init__(
        self,
        *,
        huge_tree=False,
        remove_comments=False,
        remove_pis=False,
        strip_cdata=True,
        resolve_entities=True,
        no_network=True,
        recover=False,
        dtd_validation=False,
        load_dtd=False,
        ns_clean=False,
        encoding=None,
        max_depth=None,
        max_entity_expansion=None,
        namespace_aware=None,
        forbid_dtd=False,
        forbid_entities=False,
        collect_ids=True,
        compact=True,
        **kwargs,
    ):
        """Validate and store parser options.

        Options that map onto uppsala's parser (``huge_tree``, ``max_depth``,
        ``max_entity_expansion``, ``namespace_aware``, ``forbid_dtd``,
        ``forbid_entities``) and post-parse transforms (``remove_comments``,
        ``remove_pis``, ``strip_cdata``) are honored. ``forbid_dtd`` rejects any
        ``<!DOCTYPE`` at parse time and ``forbid_entities`` rejects ``<!ENTITY>``
        declarations (defusedxml-style hardening). Options whose absence would
        silently change correctness raise ``NotImplementedError``; purely
        cosmetic options are accepted and ignored.
        """
        if recover:
            raise NotImplementedError("recover-mode parsing is not supported")
        if dtd_validation or load_dtd:
            raise NotImplementedError("DTD processing is not supported")
        if not resolve_entities:
            raise NotImplementedError("resolve_entities=False is not supported")
        target = kwargs.pop("target", None)
        resolvers = kwargs.pop("resolvers", None)
        if kwargs:
            names = ", ".join(sorted(kwargs))
            raise TypeError("unexpected XMLParser keyword argument(s): %s" % names)
        if target is not None:
            raise NotImplementedError("custom parser targets are not supported")
        if resolvers is not None:
            raise NotImplementedError("custom URI resolvers are not supported")
        self._opts = {
            "huge_tree": huge_tree,
            "remove_comments": remove_comments,
            "remove_pis": remove_pis,
            "strip_cdata": strip_cdata,
            "max_depth": max_depth,
            "max_entity_expansion": max_entity_expansion,
            "namespace_aware": namespace_aware,
            "forbid_dtd": forbid_dtd,
            "forbid_entities": forbid_entities,
            # Honored for byte input: overrides the document's declared encoding
            # by decoding in Python before parsing (see fromstring).
            "encoding": encoding,
        }


def _parse_kwargs(opts):
    """Translate stored XMLParser options into keyword args for the native parser."""
    kw = {}
    # huge_tree lifts the safe defaults; explicit limits then take precedence.
    if opts.get("huge_tree"):
        kw["max_depth"] = _HUGE_DEPTH
        kw["max_entity_expansion"] = _HUGE_ENTITY
    if opts.get("max_depth") is not None:
        kw["max_depth"] = opts["max_depth"]
    if opts.get("max_entity_expansion") is not None:
        kw["max_entity_expansion"] = opts["max_entity_expansion"]
    if opts.get("namespace_aware") is not None:
        kw["namespace_aware"] = opts["namespace_aware"]
    if opts.get("forbid_dtd"):
        kw["forbid_dtd"] = True
    if opts.get("forbid_entities"):
        kw["forbid_entities"] = True
    return kw


def _postprocess(holder, opts):
    """Apply post-parse tree transforms requested by XMLParser options."""
    root = holder.doc.document_element
    if root is None:
        return
    stripping = opts.get("remove_comments") or opts.get("remove_pis")
    if stripping:
        _strip_kinds(
            holder,
            root,
            remove_comments=opts.get("remove_comments"),
            remove_pis=opts.get("remove_pis"),
        )
    strip_cdata = opts.get("strip_cdata", True)
    if strip_cdata:
        _convert_cdata(holder, root)
    if stripping or strip_cdata:
        # Removing a comment/PI can leave the text that surrounded it split
        # across two adjacent text nodes; converting CDATA can do the same.
        # Merge them so .text/.tail expose a single contiguous run, matching
        # lxml where the removed/converted node does not split plain text.
        _coalesce_text(holder, root)


def _strip_kinds(holder, node, remove_comments, remove_pis):
    """Recursively remove comment and/or PI children from the subtree."""
    for child in list(node.children):
        kind = child.kind
        if (kind == "comment" and remove_comments) or (
            kind == "processing_instruction" and remove_pis
        ):
            holder.doc.remove_child(node, child)
        elif kind == "element":
            _strip_kinds(holder, child, remove_comments, remove_pis)


def _coalesce_text(holder, node):
    """Merge adjacent plain-text sibling nodes into one, recursing into elements.

    Only plain ``text`` nodes are merged; CDATA sections are left as distinct
    nodes so that ``strip_cdata=False`` preserves them verbatim. The text of
    each run after the first is appended to the first node and the extra nodes
    are removed.
    """
    run_head = None
    for child in list(node.children):
        kind = child.kind
        if kind == "text":
            if run_head is None:
                run_head = child
            else:
                run_head.set_text((run_head.text or "") + (child.text or ""))
                holder.doc.remove_child(node, child)
        else:
            run_head = None
            if kind == "element":
                _coalesce_text(holder, child)


def _convert_cdata(holder, node):
    """Recursively replace CDATA nodes with plain text nodes (lxml's default)."""
    for child in list(node.children):
        if child.kind == "cdata":
            tn = holder.doc.create_text(child.text or "")
            holder.doc.replace_child(node, tn, child)
        elif child.kind == "element":
            _convert_cdata(holder, child)


def fromstring(text, parser=None):
    """Parse an XML string (or bytes) and return its root element.

    A parser ``encoding`` (if set) overrides the document's declared encoding
    for byte input: the bytes are decoded with it before parsing. It has no
    effect on ``str`` input, which is already decoded.
    """
    opts = parser._opts if parser is not None else {}
    kw = _parse_kwargs(opts)
    encoding = opts.get("encoding")
    try:
        if isinstance(text, (bytes, bytearray)):
            if encoding:
                # Honor the parser's encoding override by decoding here; uppsala
                # parses the resulting str regardless of any declared encoding.
                decoded = bytes(text).decode(encoding)
                if decoded[:1] == "\ufeff":
                    decoded = decoded[1:]  # drop a leading BOM
                doc = _u.parse(decoded, **kw)
            else:
                doc = _u.parse_bytes(bytes(text), **kw)
        else:
            doc = _u.parse(text, **kw)
    except (LookupError, UnicodeDecodeError) as e:
        # Unknown codec name or bytes that do not decode under it.
        raise XMLSyntaxError(str(e)) from e
    except (
        _u.XmlParseError,
        _u.XmlWellFormednessError,
        _u.XmlNamespaceError,
    ) as e:
        # Re-raise native parse failures under the lxml-compatible name.
        raise XMLSyntaxError(str(e)) from e
    holder = _DocHolder(doc)
    if opts:
        _postprocess(holder, opts)
    root = doc.document_element
    if root is None:
        raise XMLSyntaxError("Document has no root element")
    return holder.proxy(root)


XML = fromstring


def fromstringlist(strings, parser=None):
    """Parse XML supplied as an iterable of string or bytes fragments."""
    # Materialize first so generators/iterators (not just sequences) work.
    fragments = list(strings)
    if not fragments:
        raise XMLSyntaxError("empty input")
    if isinstance(fragments[0], (bytes, bytearray)):
        return fromstring(b"".join(bytes(s) for s in fragments), parser)
    return fromstring("".join(fragments), parser)


def _read_source(source):
    """Read parse input from a filename/path or a file-like object.

    Matching lxml/ElementTree, a ``str``, ``bytes`` or ``os.PathLike`` is always
    treated as a filesystem path; an object with a ``read`` method is read as a
    file. In-memory XML strings/bytes go through :func:`fromstring`, not here.
    """
    if hasattr(source, "read"):
        return source.read()
    with open(source, "rb") as fh:
        return fh.read()


def parse(source, parser=None, base_url=None):
    """Parse from a filename/path or file-like ``source`` into an ElementTree.

    As in ``lxml.etree``, ``source`` is interpreted as a filesystem path (when a
    ``str``/``bytes``/``os.PathLike``) or as a file-like object; it is **not**
    treated as inline XML. To parse an in-memory string or bytes use
    :func:`fromstring`, or wrap it in ``io.BytesIO``/``io.StringIO``.

    ``base_url`` records the document's base for resolving relative XInclude
    ``href`` references in a later :meth:`_ElementTree.xinclude` call. When not
    given, a filesystem ``source`` path is used as the base.
    """
    data = _read_source(source)
    el = fromstring(data, parser)
    if base_url is None and isinstance(source, (str, bytes, os.PathLike)):
        base_url = os.fspath(source)
        if isinstance(base_url, bytes):
            base_url = base_url.decode("utf-8", "replace")
    el._holder.base_url = base_url
    return _ElementTree(el._holder, el._node)


def _tostring_open_tag_end(text):
    """Index of the ``>`` (or the ``/`` of ``/>``) that closes the first start
    tag in ``text``, respecting quoted attribute values; ``None`` if not found.
    """
    quote = None
    for i, c in enumerate(text):
        if quote:
            if c == quote:
                quote = None
        elif c in ('"', "'"):
            quote = c
        elif c == ">":
            return i - 1 if i > 0 and text[i - 1] == "/" else i
    return None


def _inject_inherited_namespaces(element, text):
    """Add namespace declarations the element inherits from its ancestors to the
    serialized top start tag, so a serialized sub-element round-trips (matching
    lxml). A no-op for the document root, whose in-scope map equals its own
    declarations.
    """
    nsmap = element.nsmap
    if not nsmap:
        return text
    own = {(pfx or None) for pfx, _uri in element._node.namespace_declarations}
    missing = [(pfx, uri) for pfx, uri in nsmap.items() if pfx not in own]
    if not missing:
        return text
    decls = "".join(
        ' xmlns="%s"' % uri if pfx is None else ' xmlns:%s="%s"' % (pfx, uri)
        for pfx, uri in missing
    )
    insert_at = _tostring_open_tag_end(text)
    if insert_at is None:
        return text
    return text[:insert_at] + decls + text[insert_at:]


def tostring(
    element_or_tree,
    encoding=None,
    method="xml",
    xml_declaration=None,
    pretty_print=False,
    doctype=None,
    **kwargs,
):
    """Serialize an element or tree to XML.

    Returns ``str`` when ``encoding="unicode"``, otherwise ``bytes`` (default
    encoding is ASCII with no XML declaration, like lxml). ``pretty_print=True``
    indents the output.

    ``doctype`` lets the caller inject a custom ``<!DOCTYPE ...>`` string ahead
    of the root element (matching lxml). When serializing a whole
    :class:`_ElementTree` and no explicit ``doctype`` is given, the DOCTYPE
    preserved on the parsed document is emitted automatically; serializing a
    bare element never emits one, also matching lxml.

    Only ``method="xml"`` is supported; other lxml serialization methods
    (``"html"``, ``"text"``, ``"c14n"``) change the output semantics and raise
    ``NotImplementedError`` rather than being silently ignored.

    Any other keyword argument (an unsupported lxml option or a typo) is
    rejected with ``TypeError`` rather than being silently dropped.
    """
    if kwargs:
        names = ", ".join(sorted(kwargs))
        raise TypeError("unexpected tostring keyword argument(s): %s" % names)
    if method not in (None, "xml"):
        raise NotImplementedError(
            "tostring(method=%r) is not supported; only 'xml' is available" % method
        )
    if isinstance(element_or_tree, _ElementTree):
        tree = element_or_tree
        element = tree.getroot()
    else:
        tree = None
        element = element_or_tree
    if element is None:
        raise AssertionError("ElementTree not initialized, missing root")

    # Resolve the DOCTYPE to emit. An explicit ``doctype`` argument always wins;
    # otherwise a serialized *tree* round-trips the DOCTYPE preserved on its
    # document. A bare element never carries a DOCTYPE, matching lxml.
    doctype_str = doctype
    if doctype_str is None and tree is not None:
        doctype_str = tree.docinfo.doctype or None

    node = element._node
    if pretty_print:
        text = node.to_xml_with_options("  ", False)
        if not text.endswith("\n"):
            text += "\n"
    else:
        text = node.to_xml()

    # uppsala's serializer emits only the namespace declarations made *on* the
    # serialized element, not those it inherits from ancestors. Serializing a
    # sub-element that uses an inherited prefix (or default namespace) would then
    # drop the binding and produce a fragment that cannot reparse. lxml keeps
    # in-scope declarations on the serialization root, so mirror that by adding
    # any inherited-but-undeclared bindings to the top start tag. For a document
    # root this is a no-op (its nsmap equals its own declarations).
    text = _inject_inherited_namespaces(element, text)

    # The DOCTYPE sits between the optional XML declaration and the root, so
    # prepend it before the declaration logic below (which prepends in turn).
    if doctype_str:
        text = doctype_str + "\n" + text

    if encoding is not None and str(encoding).lower() == "unicode":
        if xml_declaration:
            text = '<?xml version="1.0"?>\n' + text
        return text

    # Byte output. Default (encoding=None) is ASCII with no declaration, like lxml.
    enc = "ASCII" if encoding is None else str(encoding)
    if xml_declaration is None:
        xml_declaration = encoding is not None and enc.lower() not in (
            "utf-8",
            "us-ascii",
            "ascii",
        )
    if xml_declaration:
        decl = '<?xml version="1.0" encoding="%s"?>\n' % enc
        text = decl + text
    return text.encode(enc, "xmlcharrefreplace")


def tounicode(element_or_tree, **kwargs):
    """Serialize to a ``str`` (shorthand for ``tostring(..., encoding="unicode")``)."""
    kwargs["encoding"] = "unicode"
    return tostring(element_or_tree, **kwargs)


def dump(elem, *, pretty_print=True, **kwargs):
    """Write a debug serialization of ``elem`` to stdout.

    Extra keyword arguments are forwarded to :func:`tostring` (so options such
    as ``xml_declaration`` take effect and unsupported options are rejected),
    while ``encoding="unicode"`` is always used for stdout output.
    """
    if isinstance(elem, _ElementTree):
        elem = elem.getroot()
    kwargs["encoding"] = "unicode"
    kwargs.setdefault("pretty_print", pretty_print)
    print(tostring(elem, **kwargs), end="")


def indent(tree, space="  ", level=0):
    """Add whitespace to a tree's text/tail for pretty-printing in place.

    Ported from ``xml.etree.ElementTree.indent``.
    """
    if isinstance(tree, _ElementTree):
        tree = tree.getroot()
    if tree is None:
        return
    indentations = ["\n" + level * space]

    def _indent(elem, level):
        child_level = level + 1
        try:
            child_indent = indentations[child_level]
        except IndexError:
            child_indent = indentations[level] + space
            indentations.append(child_indent)

        children = list(elem)
        if not children:
            return
        if not elem.text or not elem.text.strip():
            elem.text = child_indent
        for child in children:
            if len(child):
                _indent(child, child_level)
            if not child.tail or not child.tail.strip():
                child.tail = child_indent
        # dedent the last child's tail
        if not children[-1].tail or not children[-1].tail.strip():
            children[-1].tail = indentations[level]

    _indent(tree, level)


# ---------------------------------------------------------------------------
# XPath helpers (precompiled)
# ---------------------------------------------------------------------------


class XPath:
    """A reusable, precompiled XPath expression callable on elements/trees."""

    def __init__(self, path, namespaces=None, **kwargs):
        # No extra options (lxml's regexp/smart_strings/extensions) are
        # supported; reject unknown kwargs rather than silently dropping them,
        # matching XMLParser/tostring strictness.
        if kwargs:
            names = ", ".join(sorted(kwargs))
            raise TypeError("unexpected XPath keyword argument(s): %s" % names)
        self.path = path
        self._namespaces = namespaces

    def __call__(self, element_or_tree, **variables):
        """Evaluate the expression against ``element_or_tree``."""
        if isinstance(element_or_tree, _ElementTree):
            element_or_tree = element_or_tree.getroot()
        # Forward variables so unsupported variable binding raises rather than
        # being silently dropped.
        return element_or_tree.xpath(
            self.path, namespaces=self._namespaces, **variables
        )


class ETXPath(XPath):
    """XPath with ElementTree ``{namespace}tag`` notation (treated as XPath)."""


def XPathEvaluator(element_or_tree, namespaces=None, **kwargs):
    """Return a callable that evaluates XPath expressions against a fixed context."""
    # No extra options are supported here; reject unknown kwargs rather than
    # silently dropping them, matching XMLParser/tostring strictness.
    if kwargs:
        names = ", ".join(sorted(kwargs))
        raise TypeError("unexpected XPathEvaluator keyword argument(s): %s" % names)
    root = (
        element_or_tree.getroot()
        if isinstance(element_or_tree, _ElementTree)
        else element_or_tree
    )

    def evaluate(path, **variables):
        # Forward variables so unsupported variable binding raises rather than
        # being silently dropped.
        return root.xpath(path, namespaces=namespaces, **variables)

    return evaluate


# ---------------------------------------------------------------------------
# XSD validation
# ---------------------------------------------------------------------------


class XMLSchema:
    """An XSD schema validator wrapping :class:`pyuppsala.XsdValidator`.

    Build from a parsed schema element (``XMLSchema(schema_root)``) or a schema
    file (``XMLSchema(file=...)``). As with the native validator, the schema must
    not include an ``<?xml ...?>`` declaration.

    Pass ``lenient=True`` to enable libxml2/lxml-compatible built-in datatype
    validation (wraps the native :meth:`pyuppsala.XsdValidator.set_lenient`).
    This is off by default (strict); turning it on notably makes ``anyURI``
    values containing a space valid, as libxml2/lxml accept them, which is
    needed to match lxml on real-world documents such as SAML metadata.
    """

    def __init__(self, etree=None, *, file=None, base_path=None, lenient=False):
        # ``base_path`` (a directory) lets the native validator resolve
        # ``xsd:import``/``xsd:include`` ``schemaLocation`` references. When a
        # filesystem ``file`` is given it defaults to that file's directory,
        # matching lxml's resolution of relative imports against the schema file.
        if etree is not None:
            schema_xml = tostring(etree, encoding="unicode")
        elif file is not None:
            if hasattr(file, "read"):
                schema_xml = file.read()
                if isinstance(schema_xml, bytes):
                    schema_xml = schema_xml.decode("utf-8")
            else:
                with open(file, "r", encoding="utf-8") as fh:
                    schema_xml = fh.read()
                if base_path is None:
                    base_path = os.path.dirname(os.fspath(file))
        else:
            raise XMLSchemaParseError("XMLSchema requires an etree or file argument")
        try:
            if base_path:
                self._validator = _u.XsdValidator.from_file(schema_xml, base_path)
            else:
                self._validator = _u.XsdValidator(schema_xml)
        except _u.XsdValidationError as e:
            raise XMLSchemaParseError(str(e)) from e
        if lenient:
            self._validator.set_lenient(True)
        # Populated by validate(); mirrors lxml's ``.error_log`` (best effort).
        self.error_log = []

    def validate(self, tree):
        """Return True if ``tree`` is valid; record failures in ``error_log``."""
        root = tree.getroot() if isinstance(tree, _ElementTree) else tree
        # Serialize via tostring (not raw node.to_xml) so a validated sub-element
        # keeps the namespace declarations it inherits from ancestors; otherwise
        # the standalone fragment would fail to reparse for validation.
        xml = tostring(root, encoding="unicode")
        self.error_log = self._validator.validate_str(xml)
        return len(self.error_log) == 0

    def assertValid(self, tree):
        """Raise :class:`DocumentInvalid` if ``tree`` does not validate.

        The raised exception carries an ``error_log`` list (the validation
        errors), matching lxml so callers can inspect ``ex.error_log``.
        """
        if not self.validate(tree):
            messages = "; ".join(e.message for e in self.error_log)
            exc = DocumentInvalid(messages or "Document does not validate")
            exc.error_log = self.error_log
            raise exc

    def __call__(self, tree):
        """Return True if ``tree`` validates (alias for :meth:`validate`)."""
        return self.validate(tree)


# ---------------------------------------------------------------------------
# XSLT 1.0 transformation
# ---------------------------------------------------------------------------

# Native compile/transform failures surface as these pyuppsala exceptions; map
# them onto the lxml-compatible XSLT error names.
_XSLT_NATIVE_ERRORS = (
    _u.XmlParseError,
    _u.XmlWellFormednessError,
    _u.XmlNamespaceError,
    _u.XPathError,
)


class _XSLTLogEntry:
    """A single XSLT error-log entry (best-effort lxml ``_LogEntry`` shape)."""

    def __init__(self, message):
        self.message = message
        # lxml callers (e.g. pyFF) read these attributes off each entry. We do
        # not carry libxml2's structured error fields, so report neutral values.
        self.line = 0
        self.column = 0
        self.domain = 0
        self.domain_name = "XSLT"
        self.type = 0
        self.type_name = "ERR_OK"
        self.level = 2  # XML_ERR_ERROR
        self.level_name = "ERROR"
        self.filename = "<string>"

    def __repr__(self):
        return "<_XSLTLogEntry %s>" % self.message


class _XSLTResultTree:
    """The result of applying an :class:`XSLT`.

    Mirrors lxml's ``_XSLTResultTree``: it serializes to the transformation's
    output via ``str()``/``bytes()`` and exposes the result document through
    ``getroot()`` and the usual tree methods (parsed lazily, so non-XML output
    methods such as ``text``/``html`` can still be retrieved as a string).
    """

    def __init__(self, text):
        self._text = text
        self._tree = None  # lazily parsed _ElementTree

    def _ensure_tree(self):
        if self._tree is None:
            self._tree = fromstring(self._text).getroottree()
        return self._tree

    def getroot(self):
        """Return the root element of the result document."""
        return self._ensure_tree().getroot()

    def write(self, file, **kwargs):
        """Serialize the result document to ``file`` (delegates to the tree)."""
        return self._ensure_tree().write(file, **kwargs)

    def __getattr__(self, name):
        # Delegate tree-ish access (find/findall/xpath/iter/getpath/docinfo...)
        # to the lazily parsed result document.
        return getattr(self._ensure_tree(), name)

    def __str__(self):
        return self._text

    def __bytes__(self):
        return self._text.encode("utf-8")

    def __repr__(self):
        return "<_XSLTResultTree>"


class XSLT:
    """A compiled XSLT 1.0 stylesheet, callable on an element or tree.

    Construct from a parsed stylesheet (``XSLT(stylesheet_root)``) and apply it
    with ``XSLT(...)(doc)``. The result is an :class:`_XSLTResultTree`; call
    ``str()``/``bytes()`` on it for the serialized output or ``getroot()`` for
    the result element. EXSLT extension functions are enabled (matching lxml).

    XSLT parameters (``transform(doc, name=value)``) are not yet supported and
    raise :class:`NotImplementedError`.
    """

    def __init__(self, xslt_input, *, extensions=None, regexp=True, access_control=None):
        # lxml accepts these keyword options; we do not implement custom
        # extension functions or access control, so reject them rather than
        # silently ignoring (matches XMLParser/tostring strictness).
        if extensions:
            raise NotImplementedError("XSLT extension functions are not supported")
        if access_control is not None:
            raise NotImplementedError("XSLT access control is not supported")
        # EXSLT regexp support is always enabled in the engine (lxml's default is
        # regexp=True). We cannot turn it off, so reject regexp=False rather than
        # silently ignoring a caller's explicit request to disable it.
        if not regexp:
            raise NotImplementedError("disabling EXSLT regexp support is not supported")
        stylesheet_xml = tostring(xslt_input, encoding="unicode")
        try:
            self._native = _u.Xslt(stylesheet_xml)
        except _XSLT_NATIVE_ERRORS as e:
            raise XSLTParseError(str(e)) from e
        # Mirrors lxml's ``.error_log``; populated when a transform fails.
        self.error_log = []

    def __call__(self, _input, profile_run=False, **kwargs):
        """Apply the stylesheet to ``_input`` (an element or tree)."""
        # lxml exposes ``profile_run=True`` to attach a profiling tree to the
        # result; the native engine has no profiler, so reject it rather than
        # silently returning an unprofiled result.
        if profile_run:
            raise NotImplementedError("XSLT profile_run is not supported")
        if kwargs:
            names = ", ".join(sorted(kwargs))
            raise NotImplementedError(
                "XSLT parameters are not yet supported: %s" % names
            )
        source_xml = tostring(_input, encoding="unicode")
        try:
            result = self._native.transform(source_xml)
        except _XSLT_NATIVE_ERRORS as e:
            self.error_log = [_XSLTLogEntry(str(e))]
            raise XSLTApplyError(str(e)) from e
        self.error_log = []
        return _XSLTResultTree(result)

    @staticmethod
    def strparam(value):
        """Wrap a string as an XSLT string parameter (lxml-compatible helper)."""
        # lxml returns an opaque token quoting the value for use as a param.
        # Parameters are not yet wired through to the engine, but provide the
        # helper so call sites that build params do not break at import time.
        return "'%s'" % str(value).replace("'", "&apos;")


# ---------------------------------------------------------------------------
# XInclude 1.0 processing
# ---------------------------------------------------------------------------

XINCLUDE_NS = "http://www.w3.org/2001/XInclude"
_XI_INCLUDE = "{%s}include" % XINCLUDE_NS
_XI_FALLBACK = "{%s}fallback" % XINCLUDE_NS
_XINCLUDE_MAX_DEPTH = 250
# Seconds to wait on a remote XInclude fetch before giving up (anti-hang).
_XINCLUDE_NETWORK_TIMEOUT = 30


def _xinclude_resolve(href, base_url):
    """Resolve an XInclude ``href`` against ``base_url`` (URL or filesystem)."""
    import os
    import urllib.parse

    # Only a real network/file URL scheme counts as "absolute". A single-letter
    # "scheme" reported by urlparse is a Windows drive letter (e.g. ``C:``), not
    # a URL scheme, so it must not be treated as one.
    url_schemes = ("http", "https", "ftp", "file")
    if urllib.parse.urlparse(href).scheme in url_schemes:
        return href  # already an absolute URL
    if base_url and urllib.parse.urlparse(base_url).scheme in url_schemes:
        # The base is a URL: resolve with URL join semantics.
        return urllib.parse.urljoin(base_url, href)
    # Filesystem paths (the common case, and the Windows case). urljoin must not
    # be used here: on Windows it misreads the drive letter as a URL scheme and
    # does not treat ``\`` as a separator, so it would drop the base directory
    # and return ``href`` unchanged. Resolve with os.path instead.
    if os.path.isabs(href):
        return href
    if base_url:
        # ``base_url`` is normally the including document's file path, so relatives
        # resolve against its directory. If a caller passes an actual directory,
        # resolve against it directly rather than its parent.
        base_dir = base_url if os.path.isdir(base_url) else os.path.dirname(base_url)
        return os.path.join(base_dir, href)
    return href  # relative to the current working directory


def _xinclude_read_bytes(resolved, allow_network=False):
    """Fetch the bytes for a resolved XInclude target (http(s)/ftp/file/path)."""
    import urllib.parse
    import urllib.request

    parts = urllib.parse.urlparse(resolved)
    if parts.scheme in ("http", "https", "ftp"):
        # Remote fetches are opt-in (see _Element.xinclude): refusing them by
        # default prevents SSRF / data exfiltration when XInclude is applied to
        # untrusted XML. Raising here lets any xi:fallback take over.
        if not allow_network:
            raise XIncludeError(
                "remote XInclude fetch of %r requires network_access=True"
                % resolved
            )
        # Bound the fetch so a slow/unresponsive remote target cannot hang
        # processing indefinitely (important for batch/service use).
        with urllib.request.urlopen(  # noqa: S310
            resolved, timeout=_XINCLUDE_NETWORK_TIMEOUT
        ) as response:
            return response.read()
    if parts.scheme == "file":
        path = urllib.request.url2pathname(parts.path)
        with open(path, "rb") as fh:
            return fh.read()
    with open(resolved, "rb") as fh:
        return fh.read()


def _xinclude_insert_text(parent, idx, text):
    """Merge ``text`` into the character stream just before child ``idx``."""
    if not text:
        return
    if idx == 0:
        parent.text = (parent.text or "") + text
    else:
        prev = parent[idx - 1]
        prev.tail = (prev.tail or "") + text


def _process_xincludes(elem, base_url, _depth=0, *, allow_network=False):
    """Recursively expand ``xi:include`` directives within ``elem`` in place.

    ``_depth`` tracks the Python recursion depth (it increments on every
    tree descent, not just at ``xi:include`` boundaries) so that a pathological
    deeply nested document is rejected with :class:`XIncludeError` before it can
    exhaust Python's own recursion limit.
    """
    if _depth > _XINCLUDE_MAX_DEPTH:
        raise XIncludeError("XInclude recursion limit exceeded")
    if _depth == 0:
        # Fast pre-check at the top level: if the subtree contains no
        # ``xi:include`` element at all, there is nothing to expand, so skip the
        # whole Python recursion. Callers (e.g. pyFF) run ``.xinclude()`` on every
        # parsed document even though most (such as SAML metadata) contain no
        # XInclude directives. ``iter`` walks natively and ``next(..., None)``
        # short-circuits on the first match (and includes ``elem`` itself), so
        # this avoids materializing a full descendant list just to test existence.
        if next(elem.iter(_XI_INCLUDE), None) is None:
            return
    # Snapshot children: the list is mutated as includes are expanded.
    for child in list(elem):
        if not isinstance(child.tag, str):
            continue  # comment / processing instruction
        # Descending to a child is one level deeper regardless of whether it is
        # an xi:include, so bump _depth on both branches to bound recursion.
        if child.tag == _XI_INCLUDE:
            _expand_include(elem, child, base_url, _depth + 1, allow_network)
        else:
            _process_xincludes(
                child, base_url, _depth + 1, allow_network=allow_network
            )


def _expand_include(parent, include, base_url, depth, allow_network=False):
    """Replace a single ``xi:include`` element with its referenced content."""
    href = include.get("href")
    parse_kind = include.get("parse", "xml")
    encoding = include.get("encoding")
    tail = include.tail
    idx = parent.index(include)

    data = None
    load_error = None
    try:
        if href is None:
            raise XIncludeError("xi:include without href is not supported")
        resolved = _xinclude_resolve(href, base_url)
        data = _xinclude_read_bytes(resolved, allow_network)
    except (OSError, ValueError, XIncludeError) as exc:
        load_error = exc

    # The include element itself is always removed.
    parent.remove(include)

    if load_error is not None:
        fallback = include.find(_XI_FALLBACK)
        if fallback is None:
            raise XIncludeError(
                "could not load XInclude href %r: %s" % (href, load_error)
            )
        # Expand any includes nested in the fallback, then splice its content in.
        _process_xincludes(
            fallback, base_url, depth + 1, allow_network=allow_network
        )
        _xinclude_insert_text(parent, idx, fallback.text)
        kids = list(fallback)
        for offset, kid in enumerate(kids):
            parent.insert(idx + offset, kid)
        if kids:
            kids[-1].tail = (kids[-1].tail or "") + (tail or "")
        else:
            _xinclude_insert_text(parent, idx, tail)
        return

    if parse_kind == "text":
        # Surface decode failures (bad bytes) and unknown-codec errors as
        # XIncludeError with href context, rather than leaking a raw
        # UnicodeDecodeError/LookupError to the caller.
        try:
            text = data.decode(encoding or "utf-8")
        except (UnicodeDecodeError, LookupError) as exc:
            raise XIncludeError(
                "could not decode XInclude text href %r: %s" % (href, exc)
            ) from exc
        _xinclude_insert_text(parent, idx, text + (tail or ""))
        return

    if parse_kind != "xml":
        raise XIncludeError("unsupported xi:include parse=%r" % parse_kind)

    # parse="xml": splice in the referenced document's root element, after
    # recursively expanding any includes it contains (resolved against its own
    # location). insert() deep-copies the cross-document subtree into this tree.
    included = fromstring(data)
    _process_xincludes(
        included, resolved, depth + 1, allow_network=allow_network
    )
    included.tail = tail
    parent.insert(idx, included)
