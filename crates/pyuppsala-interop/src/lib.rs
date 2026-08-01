use std::ffi::CStr;
use std::sync::{Arc, Mutex};

/// ABI version for the pyuppsala document capsule payload.
///
/// Bumped to 2 for the zero-copy [`OwnedDoc`] model (v1 wrapped an
/// `into_static()` document alongside a separately owned input `String`;
/// v2 retains the input as the document's borrowed backing storage).
pub const DOCUMENT_CAPSULE_ABI: u32 = 2;

/// PyCapsule name used for sharing pyuppsala document handles.
pub const DOCUMENT_CAPSULE_NAME: &str = "pyuppsala.document_handle.v2";

/// C-compatible form of [`DOCUMENT_CAPSULE_NAME`] for `PyCapsule` APIs.
pub const DOCUMENT_CAPSULE_CNAME: &CStr = c"pyuppsala.document_handle.v2";

/// Alias for the borrowed Uppsala document so the `self_cell!` macro can name
/// its lifetime-carrying dependent type.
type Doc<'a> = uppsala::Document<'a>;

self_cell::self_cell!(
    /// Self-referential storage: the decoded input `String` owns the bytes and
    /// the `uppsala::Document` borrows from it. `self_cell` guarantees the
    /// owner is never moved while the dependent borrows it, which lets a parsed
    /// document keep its `Cow::Borrowed` slices instead of paying the
    /// `into_static()` per-node `String` allocation (roughly 2x memory plus a
    /// transient double-arena spike).
    struct DocCell {
        owner: String,
        #[covariant]
        dependent: Doc,
    }
);

/// A `uppsala::Document` that retains and borrows from its decoded input text.
///
/// For programmatically built, imported, or XSLT-produced documents there is
/// no source text, so the owner is an empty `String` and the document owns all
/// of its data (mutations always produce `Cow::Owned` values, which coerce into
/// any lifetime).
///
/// SAFETY / SOUNDNESS: a `&mut Document` is only ever handed out inside the
/// [`OwnedDoc::with_doc_mut`] closure. Never expose it otherwise: with two
/// `OwnedDoc`s whose owners differ, a `mem::swap` of their dependents would let
/// one document outlive the input it borrows. The `for<'a>` branding on the
/// closure prevents that swap, which is why the only mutation entry point is
/// closure-scoped.
pub struct OwnedDoc {
    cell: DocCell,
}

impl OwnedDoc {
    /// Parse `input`, retaining it as the document's backing storage.
    ///
    /// The caller supplies the parse step as a closure so pyuppsala's security
    /// knobs (`max_depth`, `forbid_dtd`, ...) stay in the binding crate. On a
    /// parse error the input `String` is handed back alongside the error so the
    /// caller can reuse or report it.
    pub fn try_parse<E>(
        input: String,
        parse: impl for<'a> FnOnce(&'a str) -> Result<Doc<'a>, E>,
    ) -> Result<Self, (E, String)> {
        // self_cell hands back the recovered owner first on failure; the
        // binding wants (error, input) so callers can build a PyErr and reuse
        // the buffer, so flip the tuple here.
        match DocCell::try_new_or_recover(input, |owner| parse(owner.as_str())) {
            Ok(cell) => Ok(OwnedDoc { cell }),
            Err((owner, err)) => Err((err, owner)),
        }
    }

    /// Wrap an already-owned document (empty, built, imported, or produced by an
    /// XSLT transform). No source text is retained.
    pub fn from_owned(doc: Doc<'static>) -> Self {
        // The builder closure receives `&'x String` but returns the `'static`
        // document, which is covariant so it coerces down to `'x`.
        OwnedDoc {
            cell: DocCell::new(String::new(), |_owner| doc),
        }
    }

    /// The retained decoded input text, or `""` for owned documents.
    pub fn input(&self) -> &str {
        self.cell.borrow_owner().as_str()
    }

    /// Borrow the document immutably.
    pub fn doc(&self) -> &Doc<'_> {
        self.cell.borrow_dependent()
    }

    /// Mutate the document within a closure that also receives the input text.
    ///
    /// This is the ONLY way to obtain `&mut Document`; see the type-level
    /// soundness note.
    pub fn with_doc_mut<R>(&mut self, f: impl for<'a> FnOnce(&'a str, &mut Doc<'a>) -> R) -> R {
        self.cell
            .with_dependent_mut(|owner, dep| f(owner.as_str(), dep))
    }
}

pub type SharedDoc = Arc<Mutex<OwnedDoc>>;

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
