# quail-ooxml

The OOXML plumbing `quail-docx` and `quail-pptx` share. No steps, no fact keys.

- `package::Package`: the zip, read defensively. At most 20 000 entries, counted from the
  end-of-central-directory (and ZIP64) record before the directory is parsed; unsafe names
  (absolute, `..`, drive letters, backslashes, control characters) ignored; repeated names
  counted. The bytes every part **actually** inflates to are charged against one budget per
  file (`Limits::max_decompressed`); declared sizes are only reported. OLE-wrapped
  (encrypted) files are `Error::Encrypted`.
- `xml::walk`: one `quick-xml` event loop for every part, matching elements by local name.
  The deadline is checked every 2048 events (skipped subtrees included), one event may not
  exceed 16 MiB, a DOCTYPE is noted but never expanded (unknown entities are dropped and
  counted), and malformed XML ends only that part.
- `rels::Rels`: relationship parts, targets resolved the OPC way (never above the package
  root), at most 50 000 per part.
- `root_fields`, `parse_date`, `clean`, `Notes`: core-properties fields, W3CDTF dates, value
  cleaning, warnings and problems.
