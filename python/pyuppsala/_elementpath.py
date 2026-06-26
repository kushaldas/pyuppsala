"""find/findall (ElementPath) support for pyuppsala.etree.

Rather than vendoring a copy of CPython's ElementPath engine, this module
delegates to the standard library's ``xml.etree.ElementPath`` at runtime. That
engine is fully generic: its selectors operate on any element object that
supports iteration over children, ``.tag``, ``.iter()``, ``.get()``,
``.find()/.findall()/.iterfind()``, ``.itertext()`` and stable identity/hashing,
all of which :class:`pyuppsala.etree._Element` provides.

Only the two wildcard selectors are overridden. The stdlib builds its tree
without comment/processing-instruction children, so its ``*`` and ``//*`` steps
yield every child; pyuppsala (like lxml) keeps comments and PIs as children, so
those steps must be filtered to elements only to match lxml semantics. The
tokenizer, the (sizable) predicate parser, and the child/self/parent selectors
are reused unchanged from the standard library.

This reaches into a few private names of ``xml.etree.ElementPath`` that have
been stable across CPython releases; an import failure here would surface
immediately and clearly.
"""

from xml.etree import ElementPath as _EP

_isinstance, _str = isinstance, str


def _is_element(node):
    """True for element nodes; False for comments/PIs (whose tag is not a str)."""
    return _isinstance(node.tag, _str)


def _prepare_star(next, token):
    """``*``: select child *elements* (excluding comments/PIs), unlike stdlib."""

    def select(context, result):
        for elem in result:
            for e in elem:
                if _is_element(e):
                    yield e

    return select


def _prepare_descendant(next, token):
    """``//tag`` / ``//*``: like stdlib, but the wildcard form yields elements only."""
    try:
        token = next()
    except StopIteration:
        return
    if token[0] == "*":
        tag = "*"
    elif not token[0]:
        tag = token[1]
    else:
        raise SyntaxError("invalid descendant")

    if _EP._is_wildcard_tag(tag):
        # Namespaced wildcards ({*}*, {ns}* ...) reuse the stdlib's _prepare_tag
        # for namespace matching. Filter to elements here as well so comments/PIs
        # never reach the predicate steps, keeping the element-only contract
        # independent of stdlib internals.
        select_tag = _EP._prepare_tag(tag)

        def select(context, result):
            def select_child(result):
                for elem in result:
                    for e in elem.iter():
                        if e is not elem and _is_element(e):
                            yield e

            return select_tag(context, select_child(result))
    else:
        if tag[:2] == "{}":
            tag = tag[2:]  # '{}tag' == 'tag'

        def select(context, result):
            for elem in result:
                for e in elem.iter(tag):
                    # element-only: lxml's `//*` excludes comments/PIs
                    if e is not elem and _is_element(e):
                        yield e

    return select


# Selector table: reuse stdlib builders except for the two wildcard steps.
_ops = {
    "": _EP.prepare_child,
    "*": _prepare_star,
    ".": _EP.prepare_self,
    "..": _EP.prepare_parent,
    "//": _prepare_descendant,
    "[": _EP.prepare_predicate,
}

# Compiled-selector cache, kept separate from the stdlib's own cache.
_cache = {}


def iterfind(elem, path, namespaces=None):
    """Yield elements matching the ElementPath ``path`` relative to ``elem``.

    Mirrors ``xml.etree.ElementPath.iterfind`` but drives selection with the
    local ``_ops`` table (see module docstring) using the stdlib tokenizer.
    """
    if path[-1:] == "/":
        path = path + "*"  # implicit all

    cache_key = (path,)
    if namespaces:
        cache_key += tuple(sorted(namespaces.items()))

    try:
        selector = _cache[cache_key]
    except KeyError:
        if len(_cache) > 100:
            _cache.clear()
        if path[:1] == "/":
            raise SyntaxError("cannot use absolute path on element")
        next = iter(_EP.xpath_tokenizer(path, namespaces)).__next__
        try:
            token = next()
        except StopIteration:
            return
        selector = []
        while 1:
            try:
                selector.append(_ops[token[0]](next, token))
            except StopIteration:
                raise SyntaxError("invalid path") from None
            try:
                token = next()
                if token[0] == "/":
                    token = next()
            except StopIteration:
                break
        _cache[cache_key] = selector

    result = [elem]
    context = _EP._SelectorContext(elem)
    for select in selector:
        result = select(context, result)
    return result


def find(elem, path, namespaces=None):
    """Return the first matching element, or None."""
    return next(iterfind(elem, path, namespaces), None)


def findall(elem, path, namespaces=None):
    """Return all matching elements as a list."""
    return list(iterfind(elem, path, namespaces))


def findtext(elem, path, default=None, namespaces=None):
    """Return the text of the first match, ``""`` if it has none, else ``default``."""
    try:
        elem = next(iterfind(elem, path, namespaces))
        if elem.text is None:
            return ""
        return elem.text
    except StopIteration:
        return default
