# pyuppsala-interop

Shared Rust types for native Python extensions that operate on a
`pyuppsala.Document` without serializing and reparsing its Uppsala DOM.

This crate defines the versioned payload stored in pyuppsala's document
`PyCapsule`. Consumers must validate both `DOCUMENT_CAPSULE_CNAME` and
`DOCUMENT_CAPSULE_ABI` before accessing the payload.

The outer payload has a defined C field layout, but it contains Rust-native
types and therefore does not define a fully stable C ABI. Producers and
consumers must use compatible Rust toolchains and exactly compatible
`pyuppsala-interop` and `uppsala` versions. The crate pins its Uppsala dependency
exactly for this reason.
