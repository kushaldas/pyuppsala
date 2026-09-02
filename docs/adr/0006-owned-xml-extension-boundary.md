# ADR 0006: Replace native document capsules with an owned XML boundary

## Status

Accepted

## Context

ADR 0002 introduced a versioned Python capsule so sibling extension modules
could share pyuppsala's live Rust document. The capsule carried Rust-native
ownership and synchronization types defined by `pyuppsala-interop` across two
independently loaded Python extension shared libraries.

Downstream use produced segmentation faults at invalid instruction addresses.
Runtime capsule names and ABI numbers can validate a declared payload version,
but they cannot make Rust types, monomorphized code, allocator behavior, or
destructors safe across independently built and loaded extension modules. The
failure mode is process memory corruption rather than a recoverable Python
exception.

Serialized XML costs an extra serialization and parse for mutating operations,
but it gives the extension boundary stable ownership: each module owns every
Rust value it creates, and only Python strings cross between modules.

## Decision

Remove the `pyuppsala-interop` crate and the native document capsule API.
Keep the self-referential `OwnedDoc` implementation private to pyuppsala so the
zero-copy retained-input model from ADR 0003 remains unchanged inside this
extension.

Document-aware sibling extensions exchange owned XML through
`Document.to_xml()`. A mutating operation computes its complete result in the
consumer and, only after it succeeds, passes the result to the internal
`Document._replace_xml()` hook. Pyuppsala parses that XML into memory it owns
and replaces the current tree through Uppsala's safe document API.

Tree replacement preserves the existing document-element node identity. This
keeps existing etree root proxies attached to the updated root while proxies
for removed descendants become detached. No Rust pointer, smart pointer,
mutex, document reference, callback, or destructor crosses the shared-library
boundary.

The boundary change is released as pyuppsala 0.11.0. Consumers of the internal
replacement hook must require that version or newer.

## Consequences

- Cross-extension document exchange no longer depends on identical Rust crate
  versions, compiler output, allocator state, or shared-library lifetimes.
- The segmentation-fault class caused by destroying or dereferencing capsule
  payloads in another extension is removed.
- Mutating operations pay for serialization and parsing again. This is an
  accepted safety tradeoff; future optimization must use a genuinely stable
  boundary rather than exported Rust-native ownership.
- Replacement is applied only after the consumer has completed successfully,
  so an operation error leaves the original pyuppsala document unchanged.
- `Document._replace_xml()` remains an internal integration hook rather than a
  general public mutation API.
