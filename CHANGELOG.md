# Changelog

## Unreleased

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
