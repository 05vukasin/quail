//! `--dump-documents`: each extracted [`Document`] as one JSON line.
//!
//! The document comes from the engine's artifacts hook
//! ([`RunOptions::on_artifacts`](quail_engine::RunOptions)), so nothing is added to the
//! records' `steps`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use quail_core::{Artifacts, Document, FileRecord, Value};
use quail_engine::ArtifactsHook;
use serde::Serialize;

/// One line of a document dump. Same shape as `quail-eval`'s `inspect::SidecarLine`:
/// `{"locator": .., "blake3": "<hex>" | null, "document": <Document>}`.
#[derive(Serialize)]
struct DumpLine<'a> {
    locator: &'a str,
    blake3: Option<String>,
    document: &'a Document,
}

struct State {
    out: BufWriter<File>,
    lines: u64,
    errors: u64,
    first_error: Option<String>,
}

/// Shared writer of a document dump.
#[derive(Clone)]
pub struct DocumentDump {
    state: Arc<Mutex<State>>,
}

impl DocumentDump {
    /// Creates (or truncates) the dump file.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let f = File::create(path)?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                out: BufWriter::with_capacity(256 << 10, f),
                lines: 0,
                errors: 0,
                first_error: None,
            })),
        })
    }

    /// The engine hook writing one line per file that has a document.
    pub fn hook(&self) -> ArtifactsHook {
        let me = self.clone();
        Arc::new(move |rec: &FileRecord, artifacts: &Artifacts| {
            if let Some(doc) = artifacts.get::<Document>() {
                me.write(rec, doc);
            }
        })
    }

    /// Writes the line of `rec`'s document. Errors are kept for [`Self::finish`].
    pub fn write(&self, rec: &FileRecord, doc: &Document) {
        let line = DumpLine {
            locator: &rec.locator,
            blake3: blake3_hex(rec),
            document: doc,
        };
        // Serialize outside the lock: workers contend only for the copy.
        let buf = serde_json::to_vec(&line).map(|mut b| {
            b.push(b'\n');
            b
        });
        let mut st = self.lock();
        let r = match buf {
            Ok(b) => st.out.write_all(&b).map_err(|e| e.to_string()),
            Err(e) => Err(format!("json: {e}")),
        };
        match r {
            Ok(()) => st.lines += 1,
            Err(e) => {
                st.errors += 1;
                st.first_error
                    .get_or_insert_with(|| format!("{}: {e}", rec.locator));
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Flushes; `Err` with the first error when any line or the flush failed.
    pub fn finish(&self) -> Result<u64, String> {
        let mut st = self.lock();
        if let Err(e) = st.out.flush() {
            st.errors += 1;
            st.first_error.get_or_insert_with(|| format!("flush: {e}"));
        }
        match &st.first_error {
            None => Ok(st.lines),
            Some(e) => Err(format!("{} line(s) not written, first: {e}", st.errors)),
        }
    }
}

/// The record's `hash.blake3` as lowercase hex.
pub fn blake3_hex(rec: &FileRecord) -> Option<String> {
    rec.facts_for(quail_hash::keys::BLAKE3.as_str())
        .find_map(|f| match &f.value {
            Value::Bytes(b) => Some(hex(b)),
            _ => None,
        })
}

/// Lowercase hex.
pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}
