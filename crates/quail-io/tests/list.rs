//! `ListSource` over generated manifests.

use std::fs;
use std::io::Cursor;

use quail_core::Source;
use quail_io::ListSource;

#[test]
fn manifest_file_resolves_relative_paths_and_keeps_locators() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::create_dir_all(r.join("docs")).unwrap();
    fs::write(r.join("docs/a.pdf"), b"A").unwrap();
    fs::write(r.join("b.pdf"), b"BB").unwrap();
    let abs = r.join("docs/a.pdf");
    let manifest = format!(
        "# split: test\n\ndocs/a.pdf\r\nid-2\tb.pdf\nid-3\t{}\nmissing.pdf\nbad\t\n",
        abs.display()
    );
    fs::write(r.join("split.tsv"), manifest).unwrap();

    let mut src = ListSource::open(r.join("split.tsv")).unwrap();
    let mut out = Vec::new();
    while let Some(item) = src.next_item() {
        out.push(
            item.map(|it| (it.locator, it.blob.as_bytes().to_vec()))
                .map_err(|e| e.to_string()),
        );
    }
    assert_eq!(out.len(), 5);
    assert_eq!(out[0], Ok(("docs/a.pdf".to_owned(), b"A".to_vec())));
    assert_eq!(out[1], Ok(("id-2".to_owned(), b"BB".to_vec())));
    assert_eq!(out[2], Ok(("id-3".to_owned(), b"A".to_vec())));
    let e3 = out[3].as_ref().unwrap_err();
    assert!(e3.contains("missing.pdf"), "{e3}");
    let e4 = out[4].as_ref().unwrap_err();
    assert!(e4.contains("line 7") && e4.contains("empty path"), "{e4}");
}

#[test]
fn unreadable_entry_carries_its_locator_and_path() {
    let t = tempfile::tempdir().unwrap();
    let manifest = "id-9\tgone/missing.pdf\n";
    let mut src = ListSource::from_reader(Cursor::new(manifest), Some(t.path().to_path_buf()));
    let err = src.next_item().unwrap().unwrap_err();
    assert_eq!(err.locator(), Some("id-9"));
    let msg = err.to_string();
    assert!(
        msg.starts_with("id-9: ") && msg.contains("gone/missing.pdf"),
        "{msg}"
    );
    let quail_core::Error::Source { source, .. } = err else {
        unreachable!()
    };
    assert!(
        matches!(*source, quail_core::Error::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound),
        "{source}"
    );
    assert!(src.next_item().is_none());
}

#[test]
fn reader_with_base_and_invalid_utf8_line() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("x"), b"x").unwrap();
    let data: &[u8] = b"\xff\xfe\nx\n";
    let mut src = ListSource::from_reader(Cursor::new(data), Some(t.path().to_path_buf()));
    let first = src.next_item().unwrap().unwrap_err().to_string();
    assert!(
        first.contains("line 1") && first.contains("UTF-8"),
        "{first}"
    );
    let second = src.next_item().unwrap().unwrap();
    assert_eq!(
        (second.locator.as_str(), second.blob.as_bytes()),
        ("x", &b"x"[..])
    );
    assert!(src.next_item().is_none());
    assert!(src.next_item().is_none());
}

#[test]
fn entries_without_reading() {
    let mut src = ListSource::from_reader(Cursor::new("a\nk\tb\n"), None);
    let e: Vec<_> = std::iter::from_fn(|| src.next_entry())
        .map(Result::unwrap)
        .collect();
    assert_eq!(e[0].0, "a");
    assert_eq!(e[1], ("k".to_owned(), "b".into()));
}
