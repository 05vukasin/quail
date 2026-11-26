# Changelog

## Unreleased

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
