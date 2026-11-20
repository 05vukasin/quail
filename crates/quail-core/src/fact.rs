use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::analyzer::AnalyzerId;
use crate::document::Span;
use crate::value::Value;

/// Name of a fact: `namespace.name`, lowercase, e.g. `hash.blake3`, `pdf.producer`,
/// `signal.iban`. Each crate owns one namespace and emits only keys inside it; it declares
/// them as `pub const` items in its own `keys` module so nobody types a key twice.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FactKey(Cow<'static, str>);

impl FactKey {
    /// A key known at compile time.
    pub const fn new(key: &'static str) -> Self {
        Self(Cow::Borrowed(key))
    }

    /// The key as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part before the first dot.
    pub fn namespace(&self) -> &str {
        self.0.split_once('.').map_or(&*self.0, |(ns, _)| ns)
    }
}

impl fmt::Debug for FactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for FactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where on a page a fact was found. Zones matter: a company named in a letterhead or a
/// signature block is worth more than one mentioned mid-text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    /// Not known.
    #[default]
    Unknown,
    /// Running header / letterhead.
    Header,
    /// Running footer.
    Footer,
    /// Main text.
    Body,
    /// Title or heading.
    Title,
    /// Inside a table.
    Table,
    /// Signature block.
    Signature,
}

/// A box on a page, in PDF points (1/72 inch) from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    /// Left.
    pub x0: f32,
    /// Top.
    pub y0: f32,
    /// Right.
    pub x1: f32,
    /// Bottom.
    pub y1: f32,
}

/// Proof for a fact, precise enough to cite it and to draw it on the rendered page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Zero-based page index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// Zone of the page.
    #[serde(skip_serializing_if = "is_unknown")]
    #[serde(default)]
    pub zone: Zone,
    /// Byte range in that page's text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// Box on the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BBox>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
fn is_unknown(z: &Zone) -> bool {
    *z == Zone::Unknown
}

/// One piece of metadata about a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    /// What it is.
    pub key: FactKey,
    /// Its value.
    pub value: Value,
    /// Confidence in [0, 1]: 1 for measured facts (a hash), calibrated for inferred ones.
    pub conf: f32,
    /// The analyzer (and version) that produced it.
    pub src: AnalyzerId,
    /// Where it was found, when it was found in the content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
}
