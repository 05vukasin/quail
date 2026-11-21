#!/usr/bin/env python3
"""Write <testdata>/MANIFEST.md: measured size, file counts by extension and
real document counts (by magic bytes) for every fetched dataset.

Usage: manifest.py TESTDATA_DIR

Files are classified by content, not by name: a "real PDF" starts with %PDF,
a DOCX is a ZIP holding word/document.xml. Parquet files are opened with
pyarrow when available and their binary/image columns are sniffed the same
way, so documents embedded in parquet rows are counted too.
"""

import collections
import datetime
import json
import os
import sys
import zipfile

# name -> (source URL, license, eval-only?)
SOURCES = {
    "acord": ("https://huggingface.co/datasets/theatticusproject/acord", "CC-BY-4.0", False),
    "cuad": ("https://zenodo.org/records/4595826", "CC-BY-4.0", False),
    "doc-split": ("https://huggingface.co/datasets/amazon/doc_split", "CC-BY-NC-4.0", True),
    "doclaynet-v1.2": ("https://huggingface.co/datasets/docling-project/DocLayNet-v1.2", "CDLA-Permissive-1.0", False),
    "fintabnet-otsl": ("https://huggingface.co/datasets/docling-project/FinTabNet_OTSL", "other (CDLA-Permissive-1.0 upstream)", True),
    "gleif-lei": ("https://goldencopy.gleif.org/api/v2/golden-copies/publishes/lei2/latest", "CC0-1.0", False),
    "kleister-charity": ("https://github.com/applicaai/kleister-charity", "unclear (no license file)", True),
    "kleister-nda": ("https://github.com/applicaai/kleister-nda", "unclear (no license file)", True),
    "ledgar": ("https://huggingface.co/datasets/coastalcph/lex_glue", "CC-BY-4.0 (LexGLUE); LEDGAR upstream CC-BY-SA-4.0", False),
    "maud": ("https://huggingface.co/datasets/theatticusproject/maud", "CC-BY-4.0", False),
    "omnidocbench": ("https://huggingface.co/datasets/opendatalab/OmniDocBench", "research only, non-commercial", True),
    "postocr-sr": ("https://huggingface.co/datasets/Sagicc/postocr-sr", "CC-BY-4.0", False),
    "realkie": ("https://zenodo.org/records/13327077 (s3://project-fruitfly)", "CC-BY-NC-4.0", True),
    "rvl-cdip-n": ("https://huggingface.co/datasets/jordyvl/RVL-CDIP-N", "CC-BY-3.0 tag; source docs UCSF IDL terms (unclear)", True),
    "rvl-cdip-n-mp": ("https://huggingface.co/datasets/jordyvl/rvl_cdip_n_mp", "CC-BY-NC-4.0", True),
    "sec-material-contracts": ("https://huggingface.co/datasets/chenghao/sec-material-contracts", "CC-BY-SA-4.0", False),
    "setimes-sr": ("https://huggingface.co/datasets/classla/setimes_sr", "CC-BY-SA-4.0", False),
    "tabmepp": ("https://huggingface.co/datasets/bevaya/TABMEpp", "MIT", False),
}


def sniff(head, path=None):
    if head.startswith(b"%PDF"):
        return "pdf"
    if head.startswith(b"PK\x03\x04"):
        if path is not None:
            try:
                with zipfile.ZipFile(path) as z:
                    names = set(z.namelist())
                if "word/document.xml" in names:
                    return "docx"
            except (zipfile.BadZipFile, OSError):
                pass
        return "zip"
    if head.startswith(b"\x89PNG"):
        return "png"
    if head.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if head[:4] in (b"II*\x00", b"MM\x00*"):
        return "tiff"
    if head.startswith(b"\xd0\xcf\x11\xe0"):
        return "ole2 (doc/xls)"
    return None


def human(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1000 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.2f} {unit}"
        n /= 1000
    return str(n)


def parquet_embedded(path, counts):
    """Sniff binary columns of a parquet file; returns rows or None."""
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
    except ImportError:
        return None
    pf = pq.ParquetFile(path)
    cols = []
    for field in pf.schema_arrow:
        t = field.type
        if pa.types.is_binary(t) or pa.types.is_large_binary(t):
            cols.append((field.name, None))
        elif pa.types.is_struct(t) and any(
            f.name == "bytes" and (pa.types.is_binary(f.type) or pa.types.is_large_binary(f.type))
            for f in t
        ):
            cols.append((field.name, "bytes"))
    if not cols:
        return pf.metadata.num_rows
    for batch in pf.iter_batches(batch_size=32, columns=[c for c, _ in cols]):
        for name, sub in cols:
            arr = batch.column(name)
            if sub:
                arr = arr.field(sub)
            for v in arr.to_pylist():
                if v:
                    counts[sniff(v[:8]) or "other-binary"] += 1
    return pf.metadata.num_rows


def scan(ds_dir):
    size = 0
    by_ext = collections.Counter()
    by_magic = collections.Counter()
    embedded = collections.Counter()
    parquet_rows = 0
    dangling = 0
    for root, dirs, files in os.walk(ds_dir):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for f in files:
            if f.startswith("."):
                continue
            p = os.path.join(root, f)
            if os.path.islink(p) and not os.path.exists(p):
                dangling += 1
                continue
            st = os.stat(p)
            size += st.st_size
            ext = os.path.splitext(f)[1].lower() or "(none)"
            for double in (".tar.gz", ".json.gz", ".tsv.xz", ".csv.zip"):
                if f.lower().endswith(double):
                    ext = double
            by_ext[ext] += 1
            with open(p, "rb") as fh:
                head = fh.read(8)
            kind = sniff(head, p if head.startswith(b"PK") and ext == ".docx" else None)
            if kind in ("pdf", "docx", "ole2 (doc/xls)"):
                by_magic[kind] += 1
            if ext == ".parquet":
                rows = parquet_embedded(p, embedded)
                if rows is not None:
                    parquet_rows += rows
    # Hidden .cache dirs (hf metadata) are excluded from file counts but are on disk.
    disk = 0
    for root, _, files in os.walk(ds_dir):
        for f in files:
            p = os.path.join(root, f)
            if not os.path.islink(p):
                disk += os.stat(p).st_size
    return {
        "bytes": disk,
        "bytes_data": size,
        "files": sum(by_ext.values()),
        "by_ext": dict(by_ext.most_common()),
        "real_docs": dict(by_magic),
        "parquet_rows": parquet_rows,
        "embedded": dict(embedded.most_common()),
        "dangling_links": dangling,
        "complete": os.path.exists(os.path.join(ds_dir, ".fetch-complete")),
    }


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    base = os.path.abspath(sys.argv[1])
    results = {}
    for name in sorted(os.listdir(base)):
        d = os.path.join(base, name)
        if os.path.isdir(d) and not name.startswith("."):
            print(f"scanning {name}", file=sys.stderr)
            results[name] = scan(d)
    st = os.statvfs(base)
    free = st.f_bavail * st.f_frsize
    total = sum(r["bytes"] for r in results.values())
    out = [
        "# testdata manifest",
        "",
        f"Generated {datetime.datetime.now(datetime.timezone.utc):%Y-%m-%d %H:%M UTC} by "
        "`quail/tools/datasets/manifest.py`. Fetch scripts: `quail/tools/datasets/`.",
        "",
        f"Total on disk: **{human(total)}**. Free on filesystem: {human(free)}.",
        "",
        "Real PDF / DOCX are counted by magic bytes on disk; `embedded` counts binary",
        "payloads inside parquet rows by magic bytes.",
        "",
        "| dataset | complete | size on disk | files | real PDF | DOCX | parquet rows | embedded in parquet | license | eval-only |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for name, r in results.items():
        src, lic, eval_only = SOURCES.get(name, ("?", "?", True))
        emb = ", ".join(f"{k}: {v}" for k, v in r["embedded"].items()) or "-"
        out.append(
            f"| {name} | {'yes' if r['complete'] else 'NO'} | {human(r['bytes'])} | {r['files']} | "
            f"{r['real_docs'].get('pdf', 0)} | {r['real_docs'].get('docx', 0)} | "
            f"{r['parquet_rows'] or '-'} | {emb} | {lic} | {'yes' if eval_only else 'no'} |"
        )
    out += ["", "## Files by extension", ""]
    for name, r in results.items():
        src = SOURCES.get(name, ("?",))[0]
        exts = ", ".join(f"`{k}` {v}" for k, v in r["by_ext"].items())
        extra = f" ({r['dangling_links']} unfetched annex links)" if r["dangling_links"] else ""
        out.append(f"- **{name}** ({src}): {exts}{extra}")
    out.append("")
    with open(os.path.join(base, "MANIFEST.md"), "w") as fh:
        fh.write("\n".join(out))
    with open(os.path.join(base, "manifest.json"), "w") as fh:
        json.dump(results, fh, indent=1)
    print("\n".join(out))


if __name__ == "__main__":
    main()
