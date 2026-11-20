//! `FsSource` over generated temp trees.

use std::fs;
use std::io;
use std::path::Path;

use quail_core::{Error, Source};
use quail_io::{FsSource, ReadOptions};

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Relative locators of every Ok item, and the number of error items.
fn collect(mut src: FsSource, root: &Path) -> (Vec<String>, usize) {
    let (mut ok, mut errs) = (Vec::new(), 0);
    while let Some(item) = src.next_item() {
        match item {
            Ok(it) => {
                let rel = Path::new(&it.locator).strip_prefix(root).unwrap();
                ok.push(rel.to_string_lossy().replace('\\', "/"));
            }
            Err(_) => errs += 1,
        }
    }
    (ok, errs)
}

fn sample_tree() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(r, "a.pdf", b"%PDF-1.7 a");
    write(r, "B.PDF", b"%PDF-1.7 b");
    write(r, "notes.txt", b"hello");
    write(r, "sub/c.docx", b"PK c");
    write(r, "sub/deep/d.pdf", b"%PDF d");
    write(r, ".hidden/e.pdf", b"%PDF e");
    write(r, "sub/.f.pdf", b"%PDF f");
    write(r, "archive/old.pdf", b"%PDF old");
    t
}

#[test]
fn yields_every_file_with_bytes_and_mtime() {
    let t = sample_tree();
    let mut src = FsSource::builder().root(t.path()).build().unwrap();
    let mut n = 0;
    while let Some(item) = src.next_item() {
        let item = item.unwrap();
        let on_disk = fs::read(&item.locator).unwrap();
        assert_eq!(item.blob.as_bytes(), &on_disk[..]);
        assert!(item.modified.unwrap() > 1_600_000_000);
        n += 1;
    }
    assert_eq!(n, 8);
}

#[test]
fn sorted_walk_is_deterministic_and_byte_ordered() {
    let t = sample_tree();
    let run = || {
        collect(
            FsSource::builder()
                .root(t.path())
                .sorted(true)
                .build()
                .unwrap(),
            t.path(),
        )
        .0
    };
    let first = run();
    assert_eq!(first, run());
    assert_eq!(
        first,
        [
            ".hidden/e.pdf",
            "B.PDF",
            "a.pdf",
            "archive/old.pdf",
            "notes.txt",
            "sub/.f.pdf",
            "sub/c.docx",
            "sub/deep/d.pdf",
        ]
    );
}

#[test]
fn sorted_roots_are_sorted_and_deduplicated() {
    let t = sample_tree();
    let src = FsSource::builder()
        .root(t.path().join("sub"))
        .root(t.path().join("archive"))
        .root(t.path().join("sub"))
        .sorted(true)
        .build()
        .unwrap();
    let (got, errs) = collect(src, t.path());
    assert_eq!(errs, 0);
    assert_eq!(
        got,
        [
            "archive/old.pdf",
            "sub/.f.pdf",
            "sub/c.docx",
            "sub/deep/d.pdf"
        ]
    );
}

#[test]
fn filters() {
    let t = sample_tree();
    let r = t.path();
    let sorted = || FsSource::builder().root(r).sorted(true);

    let (got, _) = collect(
        sorted().extension("PDF").skip_hidden(true).build().unwrap(),
        r,
    );
    assert_eq!(got, ["B.PDF", "a.pdf", "archive/old.pdf", "sub/deep/d.pdf"]);

    let (got, _) = collect(
        sorted()
            .extension(".docx")
            .extension("txt")
            .build()
            .unwrap(),
        r,
    );
    assert_eq!(got, ["notes.txt", "sub/c.docx"]);

    let (got, _) = collect(
        sorted()
            .include("sub/**")
            .exclude("**/deep")
            .build()
            .unwrap(),
        r,
    );
    assert_eq!(got, ["sub/.f.pdf", "sub/c.docx"]);

    let (got, _) = collect(
        sorted()
            .exclude("archive")
            .exclude("*.pdf")
            .build()
            .unwrap(),
        r,
    );
    assert_eq!(got, ["B.PDF", "notes.txt", "sub/c.docx"]);

    let (got, _) = collect(sorted().max_depth(Some(1)).build().unwrap(), r);
    assert_eq!(got, ["B.PDF", "a.pdf", "notes.txt"]);

    assert!(FsSource::builder().include("a[").build().is_err());
}

#[test]
fn a_file_root_is_yielded_and_matched_by_name() {
    let t = sample_tree();
    let file = t.path().join("sub/c.docx");
    let (got, _) = collect(
        FsSource::builder()
            .root(&file)
            .include("*.docx")
            .build()
            .unwrap(),
        t.path(),
    );
    assert_eq!(got, ["sub/c.docx"]);
    let (got, _) = collect(
        FsSource::builder()
            .root(&file)
            .include("*.pdf")
            .build()
            .unwrap(),
        t.path(),
    );
    assert!(got.is_empty());
}

#[test]
fn missing_root_is_one_error_and_the_walk_goes_on() {
    let t = sample_tree();
    let src = FsSource::builder()
        .root(t.path().join("nope"))
        .root(t.path().join("archive"))
        .build()
        .unwrap();
    let (got, errs) = collect(src, t.path());
    assert_eq!(errs, 1);
    assert_eq!(got, ["archive/old.pdf"]);
}

#[test]
fn size_cap_turns_big_files_into_errors() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "small", &[1; 10]);
    write(t.path(), "big", &[2; 11]);
    let mut src = FsSource::builder()
        .root(t.path())
        .sorted(true)
        .read_options(ReadOptions::new().max_file_bytes(Some(10)))
        .build()
        .unwrap();
    let Some(Err(err)) = src.next_item() else {
        panic!("big should fail")
    };
    assert_eq!(
        err.locator(),
        Some(&*t.path().join("big").to_string_lossy())
    );
    let Error::Source { source, .. } = err else {
        panic!("expected a located error")
    };
    let Error::Io(e) = *source else {
        panic!("expected io")
    };
    assert_eq!(e.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(src.next_item().unwrap().unwrap().blob.len(), 10);
    assert!(src.next_item().is_none());
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_an_error_item_and_the_walk_continues() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "a", b"a");
    write(t.path(), "b", b"b");
    write(t.path(), "c", b"c");
    let b = t.path().join("b");
    fs::set_permissions(&b, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&b).is_ok() {
        eprintln!("running with privileges that ignore file modes; skipping");
        return;
    }
    let mut src = FsSource::builder()
        .root(t.path())
        .sorted(true)
        .build()
        .unwrap();
    let mut seen = Vec::new();
    while let Some(item) = src.next_item() {
        match item {
            Ok(it) => seen.push(it.blob.as_bytes().to_vec()),
            Err(Error::Source { locator, source }) => {
                assert_eq!(locator, b.to_string_lossy());
                let Error::Io(e) = *source else {
                    panic!("expected io, got {source}")
                };
                assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
                seen.push(b"ERR".to_vec());
            }
            Err(e) => panic!("unexpected {e}"),
        }
    }
    fs::set_permissions(&b, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(seen, [b"a".to_vec(), b"ERR".to_vec(), b"c".to_vec()]);
}

#[cfg(unix)]
#[test]
fn unlistable_directory_is_an_error_item() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "locked/x", b"x");
    write(t.path(), "z", b"z");
    let locked = t.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let (got, errs) = collect(
        FsSource::builder().root(t.path()).build().unwrap(),
        t.path(),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(errs, 1);
    assert_eq!(got, ["z"]);
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_by_default_and_loops_are_errors_when_followed() {
    use std::os::unix::fs::symlink;
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(r, "dir/real.pdf", b"%PDF");
    symlink(r.join("dir/real.pdf"), r.join("dir/link.pdf")).unwrap();
    symlink(r.join("dir"), r.join("dir/loop")).unwrap(); // points at its own parent

    let (got, errs) = collect(FsSource::builder().root(r).sorted(true).build().unwrap(), r);
    assert_eq!((got, errs), (vec!["dir/real.pdf".to_owned()], 0));

    let src = FsSource::builder()
        .root(r)
        .sorted(true)
        .follow_symlinks(true)
        .build()
        .unwrap();
    let (got, errs) = collect(src, r);
    assert_eq!(got, ["dir/link.pdf", "dir/real.pdf"]);
    assert_eq!(errs, 1, "the loop is reported once");
}

#[test]
fn next_path_does_not_read() {
    let t = sample_tree();
    let mut src = FsSource::builder().root(t.path()).build().unwrap();
    let mut n = 0;
    while let Some(p) = src.next_path() {
        assert!(p.unwrap().is_file());
        n += 1;
    }
    assert_eq!(n, 8);
}

#[cfg(feature = "mmap")]
#[test]
fn mmap_yields_the_same_bytes() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "empty", b"");
    write(t.path(), "small", b"abc");
    write(t.path(), "big", &vec![7; 100_000]);
    let read = |opts: ReadOptions| {
        let mut src = FsSource::builder()
            .root(t.path())
            .sorted(true)
            .read_options(opts)
            .build()
            .unwrap();
        let mut out = Vec::new();
        while let Some(item) = src.next_item() {
            out.push(item.unwrap().blob.as_bytes().to_vec());
        }
        out
    };
    assert_eq!(
        read(ReadOptions::new()),
        read(ReadOptions::new().mmap_min_bytes(Some(0)))
    );
}
