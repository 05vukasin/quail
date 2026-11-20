//! JSON Lines output: one [`FileRecord`] per line.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use quail_core::{Error, FileRecord, RecordSink, Result};

use crate::read::io_error;

/// Output buffer size: large enough that a write syscall carries many records.
const BUF: usize = 256 << 10;

type Dyn = Box<dyn Write + Send>;

enum Out {
    Plain(BufWriter<Dyn>),
    #[cfg(feature = "zstd")]
    Zstd(BufWriter<zstd::Encoder<'static, Dyn>>),
}

/// Writes one [`FileRecord`] per line as JSON (`serde_json`), through a 256 KiB buffer.
///
/// Call [`RecordSink::finish`] at the end: it flushes (and ends the zstd frame) and reports
/// any error. Dropping an unfinished sink flushes on a best-effort basis and loses errors.
/// After `finish`, `write` fails.
pub struct JsonlSink {
    out: Option<Out>,
    records: u64,
}

impl std::fmt::Debug for JsonlSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlSink")
            .field("records", &self.records)
            .field("finished", &self.out.is_none())
            .finish()
    }
}

impl JsonlSink {
    /// Writes to any writer.
    pub fn new(w: impl Write + Send + 'static) -> Self {
        Self {
            out: Some(Out::Plain(BufWriter::with_capacity(BUF, Box::new(w)))),
            records: 0,
        }
    }

    /// Creates (or truncates) a file.
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let f = File::create(path).map_err(|e| io_error(path, &e))?;
        Ok(Self::new(f))
    }

    /// Writes to standard output.
    pub fn stdout() -> Self {
        Self::new(io::stdout())
    }

    /// Writes a zstd-compressed stream (one frame) at `level` (1–22; 3 is zstd's default).
    #[cfg(feature = "zstd")]
    pub fn zstd(w: impl Write + Send + 'static, level: i32) -> Result<Self> {
        let enc = zstd::Encoder::new(Box::new(w) as Dyn, level)?;
        Ok(Self {
            out: Some(Out::Zstd(BufWriter::with_capacity(BUF, enc))),
            records: 0,
        })
    }

    /// Creates (or truncates) a zstd-compressed file, conventionally named `*.jsonl.zst`.
    #[cfg(feature = "zstd")]
    pub fn create_zstd(path: impl AsRef<Path>, level: i32) -> Result<Self> {
        let path = path.as_ref();
        let f = File::create(path).map_err(|e| io_error(path, &e))?;
        Self::zstd(f, level)
    }

    /// Records written so far.
    pub fn records(&self) -> u64 {
        self.records
    }

    fn close(&mut self) -> io::Result<()> {
        match self.out.take() {
            None => Ok(()),
            Some(Out::Plain(mut w)) => w.flush(),
            #[cfg(feature = "zstd")]
            Some(Out::Zstd(w)) => {
                let enc = w.into_inner().map_err(io::IntoInnerError::into_error)?;
                enc.finish()?.flush()
            }
        }
    }
}

fn json_error(e: serde_json::Error) -> Error {
    if e.is_io() {
        Error::Io(e.into())
    } else {
        Error::Other(format!("json: {e}"))
    }
}

fn write_line(w: &mut impl Write, record: &FileRecord) -> Result<()> {
    serde_json::to_writer(&mut *w, record).map_err(json_error)?;
    w.write_all(b"\n")?;
    Ok(())
}

impl RecordSink for JsonlSink {
    fn write(&mut self, record: &FileRecord) -> Result<()> {
        match &mut self.out {
            None => return Err(Error::Other("JsonlSink: write after finish".into())),
            Some(Out::Plain(w)) => write_line(w, record)?,
            #[cfg(feature = "zstd")]
            Some(Out::Zstd(w)) => write_line(w, record)?,
        }
        self.records += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        Ok(self.close()?)
    }
}

impl Drop for JsonlSink {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Reads records written by [`JsonlSink`] back, one per line; blank lines are skipped. A bad
/// line yields one error naming its line number and reading goes on; a read error on the
/// stream itself yields one error and ends it.
pub struct JsonlReader<R> {
    lines: R,
    line_no: usize,
    buf: String,
    done: bool,
}

impl<R> std::fmt::Debug for JsonlReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlReader")
            .field("line_no", &self.line_no)
            .finish_non_exhaustive()
    }
}

impl<R: BufRead> JsonlReader<R> {
    /// Reads from any buffered reader of plain JSONL.
    pub fn new(lines: R) -> Self {
        Self {
            lines,
            line_no: 0,
            buf: String::new(),
            done: false,
        }
    }
}

impl JsonlReader<Box<dyn BufRead + Send>> {
    /// Opens a JSONL file. With the `zstd` feature, a zstd-compressed file is recognized by
    /// its magic number and decompressed on the fly.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let f = File::open(path).map_err(|e| io_error(path, &e))?;
        #[cfg_attr(not(feature = "zstd"), allow(unused_mut))]
        let mut r = BufReader::with_capacity(BUF, f);
        #[cfg(feature = "zstd")]
        {
            const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
            let head = r.fill_buf().map_err(|e| io_error(path, &e))?;
            if head.starts_with(&MAGIC) {
                let dec = zstd::Decoder::with_buffer(r).map_err(|e| io_error(path, &e))?;
                return Ok(Self::new(Box::new(BufReader::with_capacity(BUF, dec))));
            }
        }
        Ok(Self::new(Box::new(r)))
    }
}

impl<R: BufRead> Iterator for JsonlReader<R> {
    type Item = Result<FileRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }
            self.buf.clear();
            match self.lines.read_line(&mut self.buf) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.line_no += 1;
                    // Invalid UTF-8 consumed its line; any other error may repeat forever.
                    self.done = e.kind() != io::ErrorKind::InvalidData;
                    return Some(Err(Error::Io(io::Error::new(
                        e.kind(),
                        format!("jsonl line {}: {e}", self.line_no),
                    ))));
                }
            }
            self.line_no += 1;
            let line = self.buf.trim();
            if line.is_empty() {
                continue;
            }
            return Some(
                serde_json::from_str(line)
                    .map_err(|e| Error::Other(format!("jsonl line {}: {e}", self.line_no))),
            );
        }
    }
}
