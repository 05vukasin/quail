# Changelog

## Unreleased

### Document identity ids (#123)

- `pdf.extract@4`: `pdf.xmp_document_id` and `pdf.xmp_original_document_id` (Text), the
  `xmpMM:DocumentID` and `xmpMM:OriginalDocumentID` of the catalog's XMP packet (attribute or
  element form, on a top-level `rdf:Description`; ids nested in `xmpMM:DerivedFrom`,
  `xmpMM:Pantry` or `xmpMM:Ingredients` are other documents' and are not read). As written,
  trimmed, scheme prefix kept. `xmpMM:InstanceID` is not read. The packet is decoded under the
  file's decode budget and refused above 4 MiB; the XML walk caps nesting at 64, never expands
  a DOCTYPE and checks the deadline. A malformed packet is a warning, and ids read before the
  fault are kept.
- `docx.extract@2`: `docx.w15_doc_id` (Text), the `w15:docId/@w15:val` of
  `word/settings.xml`, as written, trimmed (braces kept). `w14:docId` is not read.

### PPTX and HTML (#120)

- New extractors `pptx.extract@1` (`quail-pptx`) and `html.extract@1` (`quail-html`), in
  the default `quail` build (features `pptx` and `html`). PPTX: one page per slide, reading
  order, titles, bullets, tables, speaker notes. HTML: headings, paragraphs, lists and tables
  as blocks, layout tables read as blocks, scripts, styles, navigation and hidden content
  dropped, CSS page breaks as pages, the declared charset, inline XBRL text kept and its
  `dei:` cover facts in `html.xbrl_dei`. Both have the same hostile-input bounds as DOCX.
- `quail_core::Format` gains `Pptx` and `Html` (`"pptx"`, `"html"`). `engine.sniff@3`: a PPTX
  is now `pptx` (it was `zip` with the PPTX MIME type, and is `pptx` without
  `[Content_Types].xml` too), and text that starts like HTML is `html` (`text/html`; it was
  `text`).
- `quail-ooxml`: the zip package, XML walk and relationships of `quail-docx`, moved unchanged
  so `quail-pptx` uses the same code. `docx.extract@1` output is unchanged.

### quail-ocr

- Idle OCR engines are dropped after an idle timeout, so a long-running host gets the ONNX
  Runtime arenas of an engine back after its last scanned page (#115). The default is
  `EnginePool::DEFAULT_IDLE_TIMEOUT`, 5 minutes; set it with
  `OcrPages::with_idle_timeout(Some(d))` (`None` keeps idle engines for the life of the
  analyzer, as before) or `EnginePool::with_idle_timeout`, and drop the idle engines at once
  with `OcrPages::trim` / `EnginePool::trim`. The next page loads a new engine; the output is
  the same.
- Every PP-OCR engine runs its page on a rayon pool of its own with `intra_threads` threads, so
  `oar-ocr`'s pre- and post-processing (resize, normalization, decoding, DB post-processing)
  stays within the OCR thread budget (`jobs × threads`) and never starts rayon's global pool
  (#116). The output is byte-identical for every thread count.
