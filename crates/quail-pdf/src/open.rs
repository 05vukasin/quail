//! Opening a PDF under a decode budget, and telling caught unwinds apart.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use hayro_syntax::budget::DecodeBudget;
use hayro_syntax::{DecryptionError, LoadPdfError, Pdf, PdfData};
use quail_core::{Blob, Error, Format, Limits};

use crate::device::Abort;

/// Lets hayro read the blob without copying it.
pub(crate) struct BlobBytes(pub(crate) Blob);

impl AsRef<[u8]> for BlobBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// A fresh decode budget for `limits`: `max_decompressed` bytes in total, images up to
/// `max_render_pixels`.
pub(crate) fn budget(limits: &Limits) -> Arc<DecodeBudget> {
    Arc::new(DecodeBudget::new(
        limits.max_decompressed,
        limits.max_render_pixels,
    ))
}

/// Why a file could not be opened.
#[derive(Debug)]
pub(crate) enum OpenError {
    /// A user password is needed.
    Encrypted,
    /// Encrypted with an unsupported handler or algorithm.
    UnsupportedEncryption,
    /// The encryption dictionary is broken.
    BadEncryption,
    /// The decode budget ran out while reading the object graph.
    Budget,
    /// No usable xref or page tree, even after repair.
    Invalid,
    /// The parser panicked.
    Panic(String),
}

impl OpenError {
    /// `true` when the failure came from encryption.
    pub fn encrypted(&self) -> bool {
        matches!(
            self,
            Self::Encrypted | Self::UnsupportedEncryption | Self::BadEncryption
        )
    }
}

impl From<OpenError> for Error {
    fn from(e: OpenError) -> Self {
        match e {
            OpenError::Encrypted => Error::Encrypted,
            OpenError::UnsupportedEncryption => {
                Error::Unsupported("PDF encryption algorithm".to_owned())
            }
            OpenError::BadEncryption => Error::Malformed {
                format: Format::Pdf,
                msg: "invalid encryption dictionary".to_owned(),
            },
            OpenError::Budget => Error::LimitExceeded("max_decompressed"),
            OpenError::Invalid => Error::Malformed {
                format: Format::Pdf,
                msg: "no usable xref or page tree, even after repair".to_owned(),
            },
            OpenError::Panic(m) => Error::Panic(format!("pdf parser: {m}")),
        }
    }
}

/// Parses `blob` with every decode charged to `budget`. The xref and every object stream it
/// needs are decoded here, so a decompression bomb in the object graph is refused right away.
pub(crate) fn open(blob: &Blob, budget: &Arc<DecodeBudget>) -> Result<Pdf, OpenError> {
    let data = PdfData::from(Arc::new(BlobBytes(blob.clone()))).with_budget(budget.clone());
    match catch_unwind(AssertUnwindSafe(|| Pdf::new(data))) {
        Ok(Ok(pdf)) => Ok(pdf),
        Ok(Err(LoadPdfError::Decryption(d))) => Err(match d {
            DecryptionError::PasswordProtected => OpenError::Encrypted,
            DecryptionError::UnsupportedAlgorithm => OpenError::UnsupportedEncryption,
            DecryptionError::MissingIDEntry | DecryptionError::InvalidEncryption => {
                OpenError::BadEncryption
            }
        }),
        Ok(Err(LoadPdfError::Invalid)) if budget.limit_hit() => Err(OpenError::Budget),
        Ok(Err(LoadPdfError::Invalid)) => Err(OpenError::Invalid),
        Err(p) => Err(OpenError::Panic(panic_msg(&*p))),
    }
}

/// What a caught unwind was.
pub(crate) enum Caught {
    /// The device's watchdog stopped the interpreter.
    Abort(Abort),
    /// A real panic, with its message.
    Panic(String),
}

pub(crate) fn caught(p: &(dyn Any + Send)) -> Caught {
    match p.downcast_ref::<Abort>() {
        Some(a) => Caught::Abort(*a),
        None => Caught::Panic(panic_msg(p)),
    }
}

pub(crate) fn panic_msg(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}
