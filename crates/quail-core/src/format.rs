use serde::{Deserialize, Serialize};

/// What kind of file a blob is, decided from its bytes (magic numbers, container contents),
/// never from the file name alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Format {
    /// PDF.
    Pdf,
    /// Office Open XML word processing document.
    Docx,
    /// Office Open XML presentation.
    Pptx,
    /// HTML or XHTML (including inline XBRL).
    Html,
    /// Any other ZIP container.
    Zip,
    /// Raster image.
    Image,
    /// Plain text.
    Text,
    /// Anything else: only the generic layer (hashes, size, sketches over bytes) applies.
    Unknown,
}

impl Format {
    /// Stable lowercase name, as written in records.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Pptx => "pptx",
            Self::Html => "html",
            Self::Zip => "zip",
            Self::Image => "image",
            Self::Text => "text",
            Self::Unknown => "unknown",
        }
    }
}
