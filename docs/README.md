# quail documentation

quail turns millions of business documents (PDF, DOCX and plain text) into per-file metadata: facts with a
confidence, the step that produced them and the evidence where they were found. Offline, on a
plain CPU. Read the first three pages in order; after that, pick what you need.

## Start here

1. [What quail is](introduction.md): the problem, what you get back, what quail does not do.
2. [Quickstart](quickstart.md): build, scan a folder, inspect a file, isolation, dedup,
   profiles, OCR and the LLM summary.
3. [Concepts](concepts.md): records, facts, values, confidence, evidence, steps and versions,
   status, limits, profiles, artifacts, OCR, finalizers.

## Reference

| Page | What it covers |
|---|---|
| [Command line](cli.md) | every `quail` command, option and exit code |
| [Fact keys](keys.md) | every step and every fact key, generated from the code |

## How it works and how well

| Page | What it covers |
|---|---|
| [Architecture](architecture.md) | the crates and their dependency rules, the engine, worker isolation, the hardened PDF stack, portability |
| [Evaluation](evaluation.md) | the public datasets, the current scores and how to run `quail-eval` |
| [Performance](performance.md) | throughput and memory, and what was optimized |
| [Security](security.md) | the hostile-input model, limits, fuzzing, isolation, reporting a problem |

## Contributing

[Development](development.md): layout, rules, workflow, the evaluation gate, fuzzing and
vendored crates.

Also: [project README](../README.md)
