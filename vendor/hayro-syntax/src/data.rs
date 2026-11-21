use crate::object::ObjectIdentifier;
use crate::object::Stream;
use crate::reader::ReaderContext;
use crate::sync::FxHashMap;
use crate::sync::{Arc, Mutex, MutexExt};
use crate::util::SegmentList;
use crate::xref::ObjectStream;
use alloc::vec::Vec;
use core::fmt::{Debug, Formatter};

/// A container for the bytes of a PDF file.
#[derive(Clone)]
pub struct PdfData {
    #[cfg(feature = "std")]
    inner: Arc<dyn AsRef<[u8]> + Send + Sync>,
    #[cfg(not(feature = "std"))]
    inner: Arc<dyn AsRef<[u8]>>,
    // quail: decode budget shared by every stream decoded out of this file.
    budget: Option<Arc<crate::budget::DecodeBudget>>,
}

// quail: attach and read the decode budget.
impl PdfData {
    /// Attaches a decode budget: every stream decoded from this data is charged to it.
    pub fn with_budget(mut self, budget: Arc<crate::budget::DecodeBudget>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// The attached decode budget, if any.
    pub fn budget(&self) -> Option<&crate::budget::DecodeBudget> {
        self.budget.as_deref()
    }
}

impl Debug for PdfData {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "PdfData {{ ... }}")
    }
}

impl AsRef<[u8]> for PdfData {
    fn as_ref(&self) -> &[u8] {
        (*self.inner).as_ref()
    }
}

#[cfg(feature = "std")]
impl<T: AsRef<[u8]> + Send + Sync + 'static> From<Arc<T>> for PdfData {
    fn from(data: Arc<T>) -> Self {
        // quail: no budget by default.
        Self {
            inner: data,
            budget: None,
        }
    }
}

#[cfg(not(feature = "std"))]
impl<T: AsRef<[u8]> + 'static> From<Arc<T>> for PdfData {
    fn from(data: Arc<T>) -> Self {
        // quail: no budget by default.
        Self {
            inner: data,
            budget: None,
        }
    }
}

impl From<Vec<u8>> for PdfData {
    fn from(data: Vec<u8>) -> Self {
        Self {
            inner: Arc::new(data),
            // quail: no budget by default.
            budget: None,
        }
    }
}

/// A structure for storing the data of the PDF.
// To explain further: This crate uses a zero-parse approach, meaning that objects like
// dictionaries or arrays always store the underlying data and parse objects lazily as needed,
// instead of allocating the data and storing it in an owned way. However, the problem is that
// not all data is readily available in the original data of the PDF: Objects can also be
// stored in an object streams, in which case we first need to decode the stream before we can
// access the data.
//
// The purpose of `Data` is to allow us to access the original data as well as maybe decoded data
// by faking the same lifetime, so that we don't run into lifetime issues when dealing with
// PDF objects that actually stem from different data sources.
pub(crate) struct Data {
    data: PdfData,
    // 32 segments are more than enough as we can't have more objects than this.
    object_streams: SegmentList<ObjectStream, 32>,
    map: Mutex<FxHashMap<ObjectIdentifier, usize>>,
}

impl Debug for Data {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "Data {{ ... }}")
    }
}

impl Data {
    /// Create a new `Data` structure.
    pub(crate) fn new(data: PdfData) -> Self {
        Self {
            data,
            object_streams: SegmentList::new(),
            map: Mutex::new(FxHashMap::default()),
        }
    }

    /// Get access to the original data of the PDF.
    pub(crate) fn get(&self) -> &PdfData {
        &self.data
    }

    /// Get access to a decoded object stream.
    pub(crate) fn object_stream(
        &self,
        id: ObjectIdentifier,
        ctx: &ReaderContext<'_>,
    ) -> Option<&ObjectStream> {
        if let Some(&idx) = self.map.get().get(&id) {
            return self.object_streams.get(idx);
        }

        // Decoding may resolve other objects, so do it without holding the cache lock.
        let stream = ctx.xref().get_with::<Stream<'_>>(id, ctx)?;
        let object_stream = ObjectStream::new(&stream, None)?;

        let mut locked = self.map.get();
        if let Some(&idx) = locked.get(&id) {
            return self.object_streams.get(idx);
        }

        let idx = locked.len();
        let object_stream = self.object_streams.get_or_init(idx, || object_stream);
        locked.insert(id, idx);

        Some(object_stream)
    }
}
