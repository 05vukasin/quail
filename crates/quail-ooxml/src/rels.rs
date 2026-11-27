//! Relationship parts (`_rels/*.rels`): how parts point at each other.

use std::collections::HashMap;

use quail_core::Emitter;

use crate::Notes;
use crate::package::{Package, resolve_target};
use crate::xml::{Ev, Flow, attr_exact, name, walk};

/// One relationship of a source part.
#[derive(Clone, Debug)]
pub struct Rel {
    /// `Id`, e.g. `rId4`.
    pub id: String,
    /// Last segment of `Type`, e.g. `header`, `hyperlink`, `officeDocument`.
    pub kind: String,
    /// `Target` as written.
    pub target: String,
    /// `TargetMode="External"`.
    pub external: bool,
}

/// The relationships of one source part.
#[derive(Clone, Debug, Default)]
pub struct Rels {
    source: String,
    list: Vec<Rel>,
    by_id: HashMap<String, usize>,
}

/// Most relationships kept per part.
const MAX_RELS: usize = 50_000;

impl Rels {
    /// Reads the relationships of `source` (`""` for the package itself).
    pub fn read(pkg: &mut Package<'_>, source: &str, out: &Emitter, warn: &mut Notes) -> Self {
        let rels_name = match source.rsplit_once('/') {
            Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
            None if source.is_empty() => "_rels/.rels".to_owned(),
            None => format!("_rels/{source}.rels"),
        };
        let mut list = Vec::new();
        if let Some(part) = pkg.open_part(&rels_name) {
            let r = walk(part, out, |ev| {
                if let Ev::Start(e) | Ev::Empty(e) = ev
                    && name(e) == "Relationship"
                {
                    if list.len() >= MAX_RELS {
                        return Flow::Stop;
                    }
                    let kind = attr_exact(e, "Type").unwrap_or_default();
                    list.push(Rel {
                        id: attr_exact(e, "Id").unwrap_or_default().into_owned(),
                        kind: kind.rsplit('/').next().unwrap_or("").to_owned(),
                        target: attr_exact(e, "Target")
                            .unwrap_or_default()
                            .trim()
                            .to_owned(),
                        external: attr_exact(e, "TargetMode")
                            .is_some_and(|m| m.eq_ignore_ascii_case("external")),
                    });
                }
                Flow::Go
            });
            if let Err(stop) = r {
                warn.stop(&stop, &rels_name);
            }
        }
        let mut by_id = HashMap::with_capacity(list.len());
        for (i, r) in list.iter().enumerate() {
            by_id.entry(r.id.clone()).or_insert(i);
        }
        Self {
            source: source.to_owned(),
            list,
            by_id,
        }
    }

    /// All relationships.
    pub fn all(&self) -> &[Rel] {
        &self.list
    }

    /// Relationship by id.
    pub fn by_id(&self, id: &str) -> Option<&Rel> {
        self.by_id.get(id).and_then(|&i| self.list.get(i))
    }

    /// Resolved part name of the first internal relationship of this kind.
    pub fn part_of_kind(&self, kind: &str) -> Option<String> {
        self.list
            .iter()
            .filter(|r| !r.external && r.kind.eq_ignore_ascii_case(kind))
            .find_map(|r| resolve_target(&self.source, &r.target))
    }

    /// Resolved part name of an internal relationship by id.
    pub fn part_by_id(&self, id: &str) -> Option<String> {
        self.by_id(id)
            .filter(|r| !r.external)
            .and_then(|r| resolve_target(&self.source, &r.target))
    }
}
