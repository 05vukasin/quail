"""Writes crates/quail-cli/src/keydocs.rs from each crate's keys source.

Run from the repository root or anywhere: `python3 crates/quail-cli/tools/genkeys.py`, then
`cargo fmt -p quail-cli`. Add a crate by adding a line to SPECS (and to `registry.rs`).
"""
import os
import re

os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))

SPECS = [
    # (const name, rust path, source file, module to start at, cfg feature)
    ("ENGINE", "quail_engine::keys", "quail-engine/src/lib.rs", "keys", None),
    ("HASH", "quail_hash::keys", "quail-hash/src/lib.rs", "keys", None),
    ("DOCX", "quail_docx::keys", "quail-docx/src/keys.rs", None, "docx"),
]

HEADER = '''//! One-line docs of every fact key, per crate, for `quail keys`.
//!
//! Generated from the doc comments of each crate's `keys` module (the doc block of each
//! `pub const`). The test `every_declared_key_is_documented` fails when a crate declares a key
//! that is missing here; entries name the constants, so a renamed or removed key fails to
//! compile.

/// A fact key and its documentation.
#[derive(Clone, Debug)]
pub struct KeyDoc {
    /// The key.
    pub key: quail_core::FactKey,
    /// What it means and its value type.
    pub doc: &'static str,
}
'''


def table(name, rpath, path, mod, feature):
    src = open(path).read()
    if mod:
        src = src[src.index(f"pub mod {mod}"):]
    out = []
    doc = []
    for line in src.splitlines():
        s = line.strip()
        if s.startswith("///"):
            doc.append(s[3:].strip())
            continue
        m = re.match(r'pub const (\w+): FactKey = FactKey::new\("([^"]+)"\);', s)
        if m:
            d = " ".join(doc)
            d = re.sub(r"\[`([^`]+)`\](\([^)]*\))?", r"`\1`", d)
            d = d.replace("\\", "\\\\").replace('"', '\\"')
            out.append(f'    KeyDoc {{\n        key: {rpath}::{m.group(1)},\n        doc: "{d}",\n    }},')
        doc = []
    cfg = f'#[cfg(feature = "{feature}")]\n' if feature else ""
    return f"\n/// Keys of `{rpath}`.\n{cfg}pub const {name}: &[KeyDoc] = &[\n" + "\n".join(out) + "\n];\n"


with open("quail-cli/src/keydocs.rs", "w") as f:
    f.write(HEADER)
    for spec in SPECS:
        f.write(table(*spec))
