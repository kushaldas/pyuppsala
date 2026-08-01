# ADR 0002: Share the native document with extensions via a versioned PyCapsule

## Status

Accepted

## Context

pybergshamra (XML-DSig) historically exchanged documents with pyuppsala as
serialized XML strings. For pyFF's enveloped signing of a 100 MB SAML
aggregate that meant one full serialization on the Python side, one or more
full parses inside the signer, a full serialization of the signed result, and
a re-parse back into a pyuppsala tree. The XML work dwarfed the actual
cryptography.

Both extensions are Rust cdylibs built on the same `uppsala` DOM, owned by the
same author, and released together. What they need is shared ownership of one
live document, not a wire format.

Candidate mechanisms considered:

- A stable C ABI between the two extensions. Maximum compatibility, but it
  would freeze uppsala's internal document representation behind a hand-written
  C surface and give up Rust types entirely.
- Serialized exchange (status quo). Simple and version-proof, but pays the
  full serialize/parse round trips this change exists to remove.
- A PyCapsule carrying a Rust struct that both extensions compile against.
  Zero-copy and type-safe, at the cost of requiring compatible Rust builds on
  both sides.

## Decision

Expose `Document._bergshamra_document_capsule()`, returning a `PyCapsule` whose
payload is a `#[repr(C)] DocumentCapsule { abi: u32, shared: SharedDoc }`,
where `SharedDoc = Arc<Mutex<OwnedDoc>>` is pyuppsala's own document ownership
type. A consumer clones the `Arc` out of the capsule and then operates on the
very same DOM pyuppsala mutates, taking the document mutex for each operation
(lock order: GIL before document mutex, never the reverse).

The shared types live in a small, separately publishable crate,
`pyuppsala-interop`, so consumers name exactly the types pyuppsala uses
without depending on the pyuppsala cdylib crate itself.

Because the payload holds Rust-native types, it is deliberately NOT a stable
C ABI. Compatibility is enforced by convention plus two runtime checks that
fail loudly instead of misinterpreting memory:

- the capsule name (`pyuppsala.document_handle.v2`) is versioned, and
  consumers request it by exact name;
- the payload carries an ABI number (`DOCUMENT_CAPSULE_ABI`) that consumers
  must compare before touching the `Arc`.

Any change to the payload layout or to the underlying `uppsala` document types
bumps both the name and the number, and requires pyuppsala and its consumers
to be built against the same `pyuppsala-interop` and `uppsala` versions and be
released in lock step.

## Consequences

- pybergshamra signs and verifies pyuppsala documents in place with no
  document-sized strings and no re-parses (measured 2.2x to 2.6x faster
  signing at 1 MB to 20 MB in bergshamra's dsig benchmark).
- The method is underscore-prefixed: it is an interop hook for trusted sibling
  extensions, not public API for arbitrary callers.
- A version mismatch between pyuppsala and a consumer degrades safely: the
  capsule name or ABI check fails and the consumer can fall back to its string
  API.
- The lock-step release rule is a real operational constraint; it is recorded
  in the CHANGELOG whenever the ABI number moves (v1 to v2 happened with the
  zero-copy document model, see ADR 0003).
