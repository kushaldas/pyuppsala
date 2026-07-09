use std::ffi::CStr;
use std::sync::{Arc, Mutex};

/// ABI version for the pyuppsala document capsule payload.
pub const DOCUMENT_CAPSULE_ABI: u32 = 1;

/// PyCapsule name used for sharing pyuppsala document handles.
pub const DOCUMENT_CAPSULE_NAME: &str = "pyuppsala.document_handle.v1";

/// C-compatible form of [`DOCUMENT_CAPSULE_NAME`] for `PyCapsule` APIs.
pub const DOCUMENT_CAPSULE_CNAME: &CStr = c"pyuppsala.document_handle.v1";

/// Wraps an Uppsala document alongside the original decoded input text.
pub struct DocWithInput {
    pub doc: uppsala::Document<'static>,
    pub input: String,
}

pub type SharedDoc = Arc<Mutex<DocWithInput>>;

/// Payload stored in pyuppsala's document-handle PyCapsule.
#[repr(C)]
pub struct DocumentCapsule {
    pub abi: u32,
    pub shared: SharedDoc,
}

impl DocumentCapsule {
    pub fn new(shared: SharedDoc) -> Self {
        Self {
            abi: DOCUMENT_CAPSULE_ABI,
            shared,
        }
    }
}
