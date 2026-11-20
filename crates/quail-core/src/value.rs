use serde::{Deserialize, Serialize};

/// The value of a [`Fact`](crate::Fact). A closed set of shapes so every consumer
/// (JSON, Arrow, a query engine) can store any fact without knowing its key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Value {
    /// Yes / no.
    Bool(bool),
    /// Signed integer (counts, sizes, page numbers).
    Int(i64),
    /// Real number (ratios, scores).
    Float(f64),
    /// Text (names, identifiers as written, languages, MIME types).
    Text(Box<str>),
    /// A 64-bit hash or sketch (SimHash, xxh3).
    U64(u64),
    /// Opaque bytes, written as lowercase hex (BLAKE3 digests, MinHash signatures).
    #[serde(with = "hex_bytes")]
    Bytes(Box<[u8]>),
    /// Calendar date.
    Date(Date),
    /// Amount of money.
    Money(Money),
    /// Several values of one fact (e.g. the chunk hashes of a file).
    List(Vec<Value>),
}

impl Value {
    /// Text value from anything string-like.
    pub fn text(s: impl Into<Box<str>>) -> Self {
        Self::Text(s.into())
    }
}

/// A calendar date. Validated by whoever builds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Date {
    /// Year, e.g. 2026.
    pub y: i16,
    /// Month 1–12.
    pub m: u8,
    /// Day 1–31.
    pub d: u8,
}

/// Money as an exact decimal: `units × 10^-scale`, never a float.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    /// Amount in the smallest written unit, e.g. 420050 for 4200.50 with scale 2.
    pub units: i64,
    /// Number of decimal places in `units`.
    pub scale: u8,
    /// ISO 4217 code when known, e.g. `*b"EUR"`.
    pub currency: Option<[u8; 3]>,
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::fmt::Write as _;

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(b.len() * 2);
        for x in b {
            let _ = write!(out, "{x:02x}");
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Box<[u8]>, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() % 2 != 0 {
            return Err(serde::de::Error::custom("odd hex length"));
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip_as_hex() {
        let v = Value::Bytes(vec![0x00, 0xab, 0xff].into());
        let j = serde_json::to_string(&v).unwrap();
        assert_eq!(j, r#"{"t":"bytes","v":"00abff"}"#);
        assert_eq!(serde_json::from_str::<Value>(&j).unwrap(), v);
    }
}
