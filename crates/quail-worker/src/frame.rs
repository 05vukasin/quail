//! The wire protocol between the supervisor and a worker child, over the child's stdin and
//! stdout.
//!
//! Every message is one frame: an 12-byte header (`b"QW"`, a tag byte, a reserved zero byte,
//! the payload length as `u64` little-endian) followed by the payload. Small control
//! messages are JSON; file bytes travel raw in their own [`DATA`] frame so neither side
//! copies them into a bigger buffer.
//!
//! ```text
//! supervisor -> child   HELLO(json)                     once, right after spawn
//! child -> supervisor   READY(json) | FATAL(utf-8)
//! supervisor -> child   JOB(json) [DATA(bytes)]         DATA only for in-memory files
//! child -> supervisor   RESULT(id u64, has_doc u8, record json) [DOC(json)]
//! ```
//!
//! The child exits on end of input (the supervisor closed its stdin).

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Frame magic.
const MAGIC: [u8; 2] = *b"QW";
/// Header length.
const HEADER: usize = 12;

/// Supervisor to child: configuration.
pub(crate) const HELLO: u8 = 1;
/// Child to supervisor: engine built, ready for jobs.
pub(crate) const READY: u8 = 2;
/// Child to supervisor: cannot continue (engine build failed, protocol error).
pub(crate) const FATAL: u8 = 3;
/// Supervisor to child: one file to process.
pub(crate) const JOB: u8 = 4;
/// Supervisor to child: the bytes of the file announced by the previous [`JOB`].
pub(crate) const DATA: u8 = 5;
/// Child to supervisor: the record of one job.
pub(crate) const RESULT: u8 = 6;
/// Child to supervisor: the extracted document of the previous [`RESULT`].
pub(crate) const DOC: u8 = 7;

/// One received frame.
pub(crate) struct Frame {
    pub tag: u8,
    pub payload: Vec<u8>,
}

/// Writes one frame made of `parts` (concatenated) and flushes.
pub(crate) fn write_frame(w: &mut impl Write, tag: u8, parts: &[&[u8]]) -> io::Result<()> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut header = [0u8; HEADER];
    header[..2].copy_from_slice(&MAGIC);
    header[2] = tag;
    header[4..].copy_from_slice(&(len as u64).to_le_bytes());
    w.write_all(&header)?;
    for p in parts {
        w.write_all(p)?;
    }
    w.flush()
}

/// Reads one frame. `Ok(None)` on a clean end of input (before the first header byte).
/// A payload over `max` bytes, a bad magic or a short read is an error.
pub(crate) fn read_frame(r: &mut impl Read, max: u64) -> io::Result<Option<Frame>> {
    let mut header = [0u8; HEADER];
    let mut got = 0;
    while got < HEADER {
        match r.read(&mut header[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    if header[..2] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad frame magic {:02x?}", &header[..4]),
        ));
    }
    let mut len = [0u8; 8];
    len.copy_from_slice(&header[4..]);
    let len = u64::from_le_bytes(len);
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes over the {max} byte cap"),
        ));
    }
    let len = usize::try_from(len).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some(Frame {
        tag: header[2],
        payload,
    }))
}

/// [`HELLO`] payload.
#[derive(Serialize, Deserialize)]
pub(crate) struct Hello {
    /// `RLIMIT_AS` in bytes.
    pub rlimit_as: Option<u64>,
    /// CPU seconds per job (`RLIMIT_CPU`, re-armed before every job).
    pub rlimit_cpu_secs: Option<u64>,
    /// Stack of the processing thread.
    pub stack_size: usize,
}

/// [`READY`] payload.
#[derive(Serialize, Deserialize)]
pub(crate) struct Ready {
    /// The child's process id.
    pub pid: u32,
    /// The engine's `Limits::timeout`, in milliseconds.
    pub timeout_ms: u64,
    /// The engine's `Limits::max_bytes`.
    pub max_bytes: u64,
    /// Limits that could not be applied, one line each.
    pub notes: Vec<String>,
}

/// [`JOB`] payload.
#[derive(Serialize, Deserialize)]
pub(crate) struct JobMsg {
    pub id: u64,
    pub locator: String,
    pub modified: Option<i64>,
    pub want_document: bool,
    /// The child reads this path itself; when absent a [`DATA`] frame follows.
    pub path: Option<Vec<u8>>,
}

/// Encodes a path for [`JobMsg::path`]: raw bytes on Unix, UTF-8 (lossy) elsewhere.
pub(crate) fn path_to_bytes(p: &std::path::Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        p.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        p.to_string_lossy().into_owned().into_bytes()
    }
}

/// Inverse of [`path_to_bytes`].
#[cfg_attr(not(unix), allow(clippy::needless_pass_by_value))]
pub(crate) fn path_from_bytes(b: Vec<u8>) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b).into()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(&b).into_owned().into()
    }
}

/// Splits a [`RESULT`] payload into job id, whether a [`DOC`] frame follows, and the record
/// JSON.
pub(crate) fn split_result(payload: &[u8]) -> Option<(u64, bool, &[u8])> {
    let id = u64::from_le_bytes(payload.get(..8)?.try_into().ok()?);
    let has_doc = *payload.get(8)? != 0;
    Some((id, has_doc, payload.get(9..)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_eof() {
        let mut buf = Vec::new();
        write_frame(&mut buf, JOB, &[b"ab", b"cd"]).unwrap();
        write_frame(&mut buf, DATA, &[]).unwrap();
        let mut r = &buf[..];
        let f = read_frame(&mut r, 100).unwrap().unwrap();
        assert_eq!((f.tag, &f.payload[..]), (JOB, &b"abcd"[..]));
        let f = read_frame(&mut r, 100).unwrap().unwrap();
        assert_eq!((f.tag, f.payload.len()), (DATA, 0));
        assert!(read_frame(&mut r, 100).unwrap().is_none());
    }

    #[test]
    fn rejects_garbage_short_and_oversized() {
        let mut r = &b"hello world, not a frame"[..];
        assert!(read_frame(&mut r, 100).is_err());
        let mut buf = Vec::new();
        write_frame(&mut buf, JOB, &[&[0u8; 50]]).unwrap();
        assert!(read_frame(&mut &buf[..], 10).is_err());
        assert!(read_frame(&mut &buf[..20], 100).is_err());
        assert!(read_frame(&mut &buf[..5], 100).is_err());
    }

    #[test]
    fn result_split() {
        let mut p = 7u64.to_le_bytes().to_vec();
        p.push(1);
        p.extend_from_slice(b"{}");
        assert_eq!(split_result(&p), Some((7, true, &b"{}"[..])));
        assert_eq!(split_result(&p[..5]), None);
    }

    #[test]
    fn path_bytes_roundtrip() {
        let p = std::path::Path::new("dir/a file.pdf");
        assert_eq!(path_from_bytes(path_to_bytes(p)), p);
    }
}
