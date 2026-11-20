use std::fmt;
use std::sync::Arc;

/// The bytes of one file. Cheap to clone; owned or memory-mapped behind the same type, so
/// no crate except the source that created it knows which.
#[derive(Clone)]
pub struct Blob {
    bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
}

impl Blob {
    /// Wraps any byte container (a `Vec<u8>`, an `Arc<[u8]>`, a memory map...).
    pub fn new<T: AsRef<[u8]> + Send + Sync + 'static>(bytes: T) -> Self {
        Self {
            bytes: Arc::new(bytes),
        }
    }

    /// The file's bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        (*self.bytes).as_ref()
    }

    /// Size in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// True for an empty file.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl From<Vec<u8>> for Blob {
    fn from(v: Vec<u8>) -> Self {
        Self::new(v)
    }
}

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Blob").field("len", &self.len()).finish()
    }
}
