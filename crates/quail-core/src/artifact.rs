use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A typed intermediate result shared between analyzers: produced once, read by many,
/// never changed after it is stored. Examples: the [`Document`](crate::Document), page
/// renders for OCR, shingles of the text.
pub trait Artifact: Any + Send + Sync {
    /// Stable name used in [`Analyzer::requires`](crate::Analyzer::requires) and
    /// [`Analyzer::provides`](crate::Analyzer::provides), e.g. `"document"`.
    const NAME: &'static str;
}

/// The artifacts known about one file, by type.
#[derive(Default)]
pub struct Artifacts {
    map: HashMap<TypeId, (&'static str, Box<dyn Any + Send + Sync>)>,
}

impl Artifacts {
    /// Empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// The artifact of type `T`, if one was stored.
    pub fn get<T: Artifact>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|(_, b)| b.downcast_ref::<T>())
    }

    /// Stores an artifact. Returns `false` (and keeps the first one) if `T` is already there:
    /// artifacts are write-once.
    pub fn insert<T: Artifact>(&mut self, value: T) -> bool {
        use std::collections::hash_map::Entry;
        match self.map.entry(TypeId::of::<T>()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(v) => {
                v.insert((T::NAME, Box::new(value)));
                true
            }
        }
    }

    /// Moves every artifact of `other` in (write-once: existing ones win).
    pub fn absorb(&mut self, other: Artifacts) {
        for (k, v) in other.map {
            self.map.entry(k).or_insert(v);
        }
    }

    /// Whether an artifact with this name is present.
    pub fn has_name(&self, name: &str) -> bool {
        self.map.values().any(|(n, _)| *n == name)
    }

    /// Names of the stored artifacts.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.map.values().map(|(n, _)| *n)
    }
}

impl std::fmt::Debug for Artifacts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A(u8);
    impl Artifact for A {
        const NAME: &'static str = "a";
    }

    #[test]
    fn write_once() {
        let mut s = Artifacts::new();
        assert!(s.insert(A(1)));
        assert!(!s.insert(A(2)));
        assert_eq!(s.get::<A>().map(|a| a.0), Some(1));
        assert!(s.has_name("a"));
    }
}
