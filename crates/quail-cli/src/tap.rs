//! Getting the extracted [`Document`] out of the engine.
//!
//! The engine keeps artifacts inside `process` and returns only the record, so the CLI
//! registers a last analyzer that sees the document and hands it on: to a JSONL dump
//! (`--dump-documents`) or to `inspect --text`. It shows up in the record's `steps`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use quail_core::{Analyzer, AnalyzerId, Ctx, Document, Emitter, Error, Result, Value};
use serde::Serialize;

/// Id of the analyzer behind `--dump-documents`.
pub const DUMP_ID: AnalyzerId = AnalyzerId::new("cli.dump_documents", 1);
/// Id of the analyzer behind `inspect --text`.
pub const CAPTURE_ID: AnalyzerId = AnalyzerId::new("cli.capture", 1);

type Hook = dyn Fn(&Ctx<'_>, &Document) -> Result<()> + Send + Sync;

/// An analyzer that passes each file's document to a callback. Applies only when a document
/// was extracted, so it never adds a problem to a file without one.
pub struct DocumentTap {
    id: AnalyzerId,
    hook: Box<Hook>,
}

impl DocumentTap {
    /// A tap named `id` calling `hook`.
    pub fn new(
        id: AnalyzerId,
        hook: impl Fn(&Ctx<'_>, &Document) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id,
            hook: Box::new(hook),
        }
    }
}

impl Analyzer for DocumentTap {
    fn id(&self) -> AnalyzerId {
        self.id
    }

    fn requires(&self) -> &'static [&'static str] {
        &["document"]
    }

    fn applies(&self, ctx: &Ctx<'_>) -> bool {
        ctx.get::<Document>().is_some()
    }

    fn run(&self, ctx: &Ctx<'_>, _out: &mut Emitter) -> Result<()> {
        match ctx.get::<Document>() {
            Some(doc) => (self.hook)(ctx, doc),
            None => Ok(()),
        }
    }
}

/// One line of a document dump. Same shape as `quail-eval`'s `inspect::SidecarLine`:
/// `{"locator": .., "blake3": "<hex>" | null, "document": <Document>}`.
#[derive(Serialize)]
struct DumpLine<'a> {
    locator: &'a str,
    blake3: Option<String>,
    document: &'a Document,
}

/// Shared writer of a document dump.
#[derive(Clone)]
pub struct DocumentDump {
    out: Arc<Mutex<BufWriter<File>>>,
}

impl DocumentDump {
    /// Creates (or truncates) the dump file.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let f = File::create(path)?;
        Ok(Self {
            out: Arc::new(Mutex::new(BufWriter::with_capacity(256 << 10, f))),
        })
    }

    /// The analyzer writing one line per extracted document. Register it after
    /// `hash.identity` so the BLAKE3 digest is known.
    pub fn tap(&self) -> DocumentTap {
        let me = self.clone();
        DocumentTap::new(DUMP_ID, move |ctx, doc| me.write(ctx, doc))
    }

    fn write(&self, ctx: &Ctx<'_>, doc: &Document) -> Result<()> {
        let blake3 = ctx
            .facts
            .iter()
            .find(|f| f.key == quail_hash::keys::BLAKE3)
            .and_then(|f| match &f.value {
                Value::Bytes(b) => Some(hex(b)),
                _ => None,
            });
        let line = DumpLine {
            locator: ctx.locator,
            blake3,
            document: doc,
        };
        // Serialize outside the lock: workers contend only for the copy.
        let mut buf = serde_json::to_vec(&line).map_err(|e| Error::Other(format!("json: {e}")))?;
        buf.push(b'\n');
        let mut w = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        w.write_all(&buf)?;
        Ok(())
    }

    /// Flushes buffered lines.
    pub fn flush(&self) -> std::io::Result<()> {
        self.out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush()
    }
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

/// Keeps the last document seen (one file at a time: `inspect`).
#[derive(Clone, Default)]
pub struct Capture {
    doc: Arc<Mutex<Option<Document>>>,
}

impl Capture {
    /// The analyzer storing the document.
    pub fn tap(&self) -> DocumentTap {
        let me = self.clone();
        DocumentTap::new(CAPTURE_ID, move |_, doc| {
            *me.doc.lock().unwrap_or_else(PoisonError::into_inner) = Some(doc.clone());
            Ok(())
        })
    }

    /// The captured document, if any.
    pub fn take(&self) -> Option<Document> {
        self.doc
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
