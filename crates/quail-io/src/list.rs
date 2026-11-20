//! [`ListSource`]: files named by a manifest.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use quail_core::{Error, Result, Source, SourceItem};

use crate::read::{ReadOptions, io_error, read_file};

/// Yields the files named by a manifest, in manifest order. Used by eval to run fixed splits.
///
/// Manifest format, one entry per line (UTF-8, `\n` or `\r\n` line ends):
///
/// - `path`: the locator is the path as written;
/// - `locator<TAB>path`: an explicit locator (e.g. a stable id) for the file at `path`;
/// - empty lines and lines starting with `#` are ignored.
///
/// Relative paths are resolved against a base directory: the manifest's own directory for
/// [`ListSource::open`], the one given to [`ListSource::from_reader`] (or the working
/// directory) otherwise. Locators are kept as written, so records stay comparable when the
/// corpus moves.
///
/// Lazy: the manifest is read one line per item. A bad line (invalid UTF-8, empty path) or a
/// file that cannot be read yields one error item naming the line or path; the list goes on.
/// A read error on the manifest itself yields one error item and ends the source.
#[derive(Debug)]
pub struct ListSource<R = BufReader<File>> {
    lines: R,
    base: Option<PathBuf>,
    read: ReadOptions,
    line_no: usize,
    buf: Vec<u8>,
    done: bool,
}

impl ListSource<BufReader<File>> {
    /// Opens a manifest file; relative paths in it resolve against its directory.
    pub fn open(manifest: impl AsRef<Path>) -> Result<Self> {
        let manifest = manifest.as_ref();
        let f = File::open(manifest).map_err(|e| io_error(manifest, &e))?;
        let base = manifest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf);
        Ok(Self::from_reader(BufReader::new(f), base))
    }
}

impl<R: BufRead + Send> ListSource<R> {
    /// Reads a manifest from any buffered reader; relative paths resolve against `base`, or
    /// the working directory when `None`.
    pub fn from_reader(lines: R, base: Option<PathBuf>) -> Self {
        Self {
            lines,
            base,
            read: ReadOptions::default(),
            line_no: 0,
            buf: Vec::new(),
            done: false,
        }
    }

    /// How files are read (size cap, memory mapping).
    #[must_use]
    pub fn read_options(mut self, read: ReadOptions) -> Self {
        self.read = read;
        self
    }

    /// The next entry as `(locator, path)`, without reading the file.
    pub fn next_entry(&mut self) -> Option<Result<(String, PathBuf)>> {
        loop {
            if self.done {
                return None;
            }
            self.buf.clear();
            match self.lines.read_until(b'\n', &mut self.buf) {
                Ok(0) => {
                    self.done = true;
                    return None;
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.done = true;
                    return Some(Err(Error::Io(io::Error::new(
                        e.kind(),
                        format!("manifest after line {}: {e}", self.line_no),
                    ))));
                }
            }
            self.line_no += 1;
            match parse_line(&self.buf) {
                Ok(None) => {}
                Ok(Some((locator, path))) => {
                    let path = match &self.base {
                        Some(base) if Path::new(path).is_relative() => base.join(path),
                        _ => PathBuf::from(path),
                    };
                    return Some(Ok((locator.to_owned(), path)));
                }
                Err(msg) => {
                    return Some(Err(Error::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("manifest line {}: {msg}", self.line_no),
                    ))));
                }
            }
        }
    }
}

/// `Ok(None)` for a blank or comment line, `Ok(Some((locator, path)))` for an entry.
fn parse_line(raw: &[u8]) -> std::result::Result<Option<(&str, &str)>, &'static str> {
    let line = std::str::from_utf8(raw).map_err(|_| "not valid UTF-8")?;
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.trim().is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (locator, path) = line.split_once('\t').unwrap_or((line, line));
    if path.is_empty() {
        return Err("empty path");
    }
    if locator.is_empty() {
        return Err("empty locator");
    }
    Ok(Some((locator, path)))
}

impl<R: BufRead + Send> Source for ListSource<R> {
    fn next_item(&mut self) -> Option<Result<SourceItem>> {
        Some(self.next_entry()?.and_then(|(locator, path)| {
            let mut item = read_file(&path, &self.read)?;
            item.locator = locator;
            Ok(item)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn parses_lines() {
        assert_eq!(parse_line(b"a/b.pdf\n"), Ok(Some(("a/b.pdf", "a/b.pdf"))));
        assert_eq!(parse_line(b"id-1\tx.pdf\r\n"), Ok(Some(("id-1", "x.pdf"))));
        assert_eq!(parse_line(b"# comment\n"), Ok(None));
        assert_eq!(parse_line(b"   \n"), Ok(None));
        assert_eq!(parse_line(b""), Ok(None));
        assert_eq!(parse_line(b"id\t\n"), Err("empty path"));
        assert_eq!(parse_line(b"\tx.pdf"), Err("empty locator"));
        assert_eq!(parse_line(b"\xff\xfe\n"), Err("not valid UTF-8"));
        // A path with a space and a tab-separated path containing spaces.
        assert_eq!(
            parse_line(b"my file.pdf"),
            Ok(Some(("my file.pdf", "my file.pdf")))
        );
    }
}
