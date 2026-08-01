# ADR 0003: Zero-copy retained-input documents (drop into_static on parse)

## Status

Accepted

## Context

The uppsala parser is zero-copy: names, namespace URIs, attribute values, and
text come back as `Cow::Borrowed` slices into the input string. pyuppsala,
however, needs a `'static` document to store inside PyO3 objects, so every
parse called `Document::into_static()`, which converts each borrowed slice
into an owned per-node `String`. On top of that, pyuppsala retained the
decoded input string alongside the owned document (for `input_text`,
`Node.source`, and line/column reporting), so the text data existed twice.

Measured on an 83 MB eduGAIN aggregate (benchmarks/memstages.py,
MALLOC_ARENA_MAX=1, malloc_trim between stages):

- parse retained +342 MiB, versus lxml's +111 MiB;
- a transient double-arena spike to 916 MiB VmHWM inside `into_static()`
  (old arena, new arena, and the new owned strings coexist during the copy);
- `prepare_xpath` added +173 MiB, because virtual attribute nodes deep-clone
  owned QNames and values (borrowed Cows clone as pointer copies instead).

At pyFF's eduGAIN pipeline scale this was the single largest memory gap
against lxml.

## Decision

Make the retained input string the document's backing storage instead of a
second copy. `pyuppsala-interop` replaces
`DocWithInput { doc: Document<'static>, input: String }` with `OwnedDoc`, a
self-referential cell (the `self_cell` crate) whose owner is the decoded input
`String` and whose dependent is `uppsala::Document<'this>` borrowing from it.
Parse constructors build it with `OwnedDoc::try_parse(input, |s|
parser.parse(s))` and no longer call `into_static()`. Documents that are built
programmatically, imported, or produced by XSLT wrap an owned document via
`OwnedDoc::from_owned` with an empty owner. Mutations keep creating
`Cow::Owned` values, which coerce into the borrowed lifetime.

Soundness rules:

- the owner `String`'s heap buffer is address-stable, and `Document<'a>` is
  covariant in `'a` (enforced at compile time by self_cell's `#[covariant]`
  check);
- `&mut Document` is reachable ONLY inside `OwnedDoc::with_doc_mut`'s
  `for<'a>` closure, so a dependent can never be swapped between two cells
  with different owners;
- `Document.discard_input()` becomes a documented no-op: the input cannot be
  freed while the document borrows it. `input_text` and `Node.source` keep
  working after a call, which callers previously had to give up.

The interop capsule payload changed shape, so the document handle ABI moved to
v2 (`pyuppsala.document_handle.v2`, see ADR 0002) and `pyuppsala-interop`
became 0.2.0; pyuppsala and pybergshamra ship together.

## Consequences

- memstages on the 83 MB aggregate: parse retained +342 to +178 MiB, parse
  VmHWM 916 to 752 MiB (the double-arena spike is gone), `prepare_xpath` +173
  to +129 MiB, whole-lifecycle peak 1605 to 1383 MiB. On the pyFF eduGAIN
  build pipeline peak RSS fell from 1.74 GB to 1.56 GB, below lxml's 1.70 GB.
- A document now pins its input string for its whole lifetime. Workloads that
  parse a huge document and keep only a tiny subtree alive hold more memory
  than before; the escape hatch is copying the subtree into a fresh document
  (deepcopy / import_subtree), which produces owned data.
- Import-built aggregates still own every string (import copies), so name
  interning inside uppsala remains a separate, future memory lever.
- All 467 pyuppsala tests, including the lxml-differential identity, deepcopy,
  and cross-tree suites, pass unchanged; serialization stays byte-identical.
