<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/logo-dark.png">
    <img src="assets/logo/logo.png" alt="quail logo: a quail whose wing is a fingerprint" width="160">
  </picture>
</p>

<h1 align="center">quail</h1>

<p align="center">
  <b>Give it millions of documents. Get back rich, searchable metadata for every file, offline, on a plain CPU.</b>
</p>

---

> Early development. Nothing here is usable yet.

quail reads each file (PDF and DOCX first) and produces a record of facts about it:
exact and near-duplicate fingerprints, structure, text with positions, identifiers, dates,
amounts, references to other documents and document types. Every fact carries a confidence,
the step that produced it and where in the file it was found, so a person or an AI agent can
check it.

- **Offline.** No external API. Models ship with the library and run on the CPU.
- **Fast and small.** Written in Rust, built for millions of files on a laptop-class machine.
- **Per file.** quail describes one file at a time; grouping files into companies, contracts
  and graphs is left to the system that consumes its records.

See [docs/development.md](docs/development.md) for how it is built.

## License

MIT
