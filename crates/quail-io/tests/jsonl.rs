//! `JsonlSink` / `JsonlReader` round trips.

use std::io::{BufRead, Cursor, Write};
use std::sync::{Arc, Mutex};

use quail_core::{
    AnalyzerId, BBox, Evidence, Fact, FactKey, FileRecord, Format, RecordSink, SCHEMA_VERSION,
    Status, Timing, Value, Zone,
};
use quail_io::{JsonlReader, JsonlSink};

fn record(i: u32) -> FileRecord {
    let src = AnalyzerId::new("test.gen", 1);
    FileRecord {
        schema: SCHEMA_VERSION,
        locator: format!("dir/file-{i}.pdf"),
        size: u64::from(i) * 1000,
        format: if i.is_multiple_of(2) {
            Format::Pdf
        } else {
            Format::Docx
        },
        pages: (!i.is_multiple_of(3)).then_some(i),
        facts: vec![
            Fact {
                key: FactKey::new("test.count"),
                value: Value::Int(i64::from(i)),
                conf: 1.0,
                src,
                evidence: None,
            },
            Fact {
                key: FactKey::new("test.text"),
                value: Value::text(format!("line \"{i}\"\nwith newline and ünïcode")),
                conf: 0.75,
                src,
                evidence: Some(Evidence {
                    page: Some(0),
                    zone: Zone::Header,
                    span: None,
                    bbox: Some(BBox {
                        x0: 1.0,
                        y0: 2.0,
                        x1: 3.5,
                        y1: 4.25,
                    }),
                }),
            },
            Fact {
                key: FactKey::new("test.bytes"),
                value: Value::List(vec![
                    Value::Bytes(vec![0, 1, 254, 255].into()),
                    Value::U64(u64::MAX),
                ]),
                conf: 1.0,
                src,
                evidence: None,
            },
        ],
        steps: vec![src],
        timings: vec![Timing { src, micros: 42 }],
        status: if i.is_multiple_of(5) {
            Status::Partial {
                problems: vec!["limit".into()],
            }
        } else {
            Status::Ok
        },
        warnings: if i.is_multiple_of(4) {
            vec!["test.gen@1: odd font".into()]
        } else {
            Vec::new()
        },
    }
}

/// A writer that shares its bytes with the test.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);
impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn round_trip_through_a_file() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("out.jsonl");
    let records: Vec<_> = (0..100).map(record).collect();
    let mut sink = JsonlSink::create(&path).unwrap();
    for r in &records {
        sink.write(r).unwrap();
    }
    assert_eq!(sink.records(), 100);
    sink.finish().unwrap();
    sink.finish().unwrap(); // idempotent
    assert!(sink.write(&records[0]).is_err());

    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 100, "one record per line");
    let back: Vec<_> = JsonlReader::open(&path)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(back, records);
}

#[test]
fn nothing_is_written_until_the_buffer_fills_or_finish() {
    let shared = Shared::default();
    let mut sink = JsonlSink::new(shared.clone());
    sink.write(&record(1)).unwrap();
    assert!(shared.0.lock().unwrap().is_empty(), "buffered");
    sink.finish().unwrap();
    let bytes = shared.0.lock().unwrap().clone();
    assert!(bytes.ends_with(b"\n"));
    let back: Vec<_> = JsonlReader::new(Cursor::new(bytes))
        .map(Result::unwrap)
        .collect();
    assert_eq!(back, [record(1)]);
}

#[test]
fn drop_flushes() {
    let shared = Shared::default();
    {
        let mut sink = JsonlSink::new(shared.clone());
        sink.write(&record(2)).unwrap();
    }
    assert_eq!(
        Cursor::new(shared.0.lock().unwrap().clone())
            .lines()
            .count(),
        1
    );
}

#[test]
fn reader_reports_bad_lines_and_goes_on() {
    let good = serde_json::to_string(&record(3)).unwrap();
    let data = format!("{good}\n\nnot json\n{good}\n");
    let out: Vec<_> = JsonlReader::new(Cursor::new(data)).collect();
    assert_eq!(out.len(), 3);
    assert!(out[0].is_ok() && out[2].is_ok());
    let e = out[1].as_ref().unwrap_err().to_string();
    assert!(e.contains("line 3"), "{e}");
}

#[cfg(feature = "zstd")]
#[test]
fn zstd_round_trip() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("out.jsonl.zst");
    let records: Vec<_> = (0..50).map(record).collect();
    let mut sink = JsonlSink::create_zstd(&path, 3).unwrap();
    for r in &records {
        sink.write(r).unwrap();
    }
    sink.finish().unwrap();
    let raw = std::fs::read(&path).unwrap();
    assert_eq!(&raw[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
    let back: Vec<_> = JsonlReader::open(&path)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(back, records);
}
