# quail-fuzz

A mutation fuzzer for the whole quail pipeline that runs on the stable toolchain (cargo-fuzz
needs nightly). Not published; emits no facts.

In quail a panic is not the worst failure, because the engine catches it. What ends a scan is an
abort on allocation failure, a stack overflow, a hang or a memory blowup. So every case runs in a
**child worker process** with resource limits, and the parent classifies how it ended:

| class            | meaning                                                                 |
|------------------|-------------------------------------------------------------------------|
| `ok`             | processed, nothing remarkable                                           |
| `caught-panic`   | a step panicked and the engine caught it (still a bug)                  |
| `deadline`       | a step hit the engine's cooperative per-file deadline (`--deadline`)    |
| `mem-high`       | processed, but peak RSS above `--mem-high-mb`                           |
| `slow`           | processed, but slower than `--slow-ms`                                  |
| `oom-abort`      | `memory allocation of N bytes failed` under `RLIMIT_AS` (`--mem-mb`)    |
| `stack-overflow` | the worker's 32 MiB stack (the engine's worker size) overflowed         |
| `timeout`        | no answer within `--timeout` seconds; the harness killed it             |
| `cpu-limit`      | SIGXCPU: the case used more than `--cpu-secs` of CPU                    |
| `killed`         | SIGKILL the harness did not send (kernel OOM killer)                    |
| `abort`          | another abort, e.g. a panic that could not unwind                       |
| `signal-N`, `exit-N` | any other death                                                     |

Every step is wrapped so it writes `QF-STEP <id>` to stderr before it runs. A finding's
signature is `class@where`: the panic location for panics, and for deaths the step that was
running (`stack-overflow@docx.extract@1`). The first input of each signature is saved under
`<out>/crashes/` with a delta-debugged minimal copy (`-min`), a `.txt` with details and the
stderr tail, and a line in `crashes/summary.tsv`.

## Target

The engine as `quail-cli` wires it, in the `Deep` profile: `hash.identity`, `docx.extract`,
`text.stats`, `text.lang`, `signal.scan`, `ref.references`, `party.candidates`, plus a fuzz-only
plain-text extractor (`fuzz.text@1`: pages at form feeds, a block per line) so the text
analyzers see the tricky-Unicode inputs. With the `pdf` feature the PDF extractor is added and,
when a step provides a `Raster`, the first two pages are rendered at 36 dpi.

`pdf` needs `quail_pdf::PdfExtractor` (branch `pdf/extractor`); on master `--features pdf` does
not compile yet (TODO(quail-pdf #7)).

## Inputs and mutators

Seeds are generated (no files needed): DOCX packages from a builder (styles, numbering, tables,
headers, notes, comments, fields, tracked changes, content controls, text boxes, custom
properties), PDFs from lopdf and from raw bytes (Type0 font with a `ToUnicode` CMap, forms,
object streams, incremental updates, inline images) and text full of awkward Unicode. `--seeds
DIR` adds files.

- bytes: flips, interesting bytes and integers, inserts, deletes, duplicates, amplification,
  splices with another corpus entry, numbers replaced by boundary values;
- DOCX (taken apart and re-zipped by hand so the headers can lie): XML part mutations (OOXML
  snippets, deep nesting injection up to 10^6 levels, element repetition, hostile attribute
  values, DOCTYPE/entities, encodings, namespaces, field-char storms), dropped / duplicated /
  renamed entries, 60 000 empty entries, a 256 MiB deflate bomb, size / CRC / name lies, unknown
  methods, EOCD lies, relationship cycles and escapes, content-type swaps;
- PDF: content-stream edits (inflated, mutated, re-compressed, `/Length` and xref fixed), filter
  chains with absurd predictors, hostile constructs appended as an incremental update (2^n form
  fan-out, form / Type3 / pattern recursion, `/Contents` repeated 200 000 times, huge
  `/Width`/`/Height`, JBIG2 page size, object-stream `/N`, double-Flate bombs, deep nesting, page
  tree cycles, CMap floods, sampled functions, absurd `MediaBox`), broken xref (startxref,
  subsections, offsets, `/Prev` loops, xref streams with absurd `/W`), `/Length` lies, name swaps;
- text: hostile runs (10^5 combining marks, digits, quotes, form feeds, repeated business
  patterns), Unicode snippets, invalid UTF-8.

Inputs that produce a warning, problem, key or page count never seen before join the job's
corpus (a cheap stand-in for coverage). Everything is deterministic for a given `--seed` and
`--jobs`: job *j* uses PRNG stream *j* and its own corpus (outcomes that depend on timing aside).

## Use

```sh
CARGO_BUILD_JOBS=2 cargo build --profile fuzz -p quail-fuzz [--features pdf]
B=target/fuzz/quail-fuzz
$B fuzz --out ../lab/tmp/quail-fuzz --jobs 2 --time 3600 --seed 1    # or --iterations N
$B run ../lab/tmp/quail-fuzz/crashes/*-min.*                       # re-classify
$B exec FILE        # in this process, e.g. under gdb for a backtrace of an abort
$B minimize FILE OUT
$B seeds DIR        # write the generated seeds
```

Limits (defaults): `--mem-mb 1024` (`RLIMIT_AS`), `--cpu-secs 30` (per case, `RLIMIT_CPU` moved
before each case), `--timeout 60` (wall), `--deadline 20` (the engine's `Limits::timeout`),
`--mem-high-mb 512`, `--slow-ms 5000`, `--min-secs 120` per minimization. The `fuzz` profile
keeps debug assertions and overflow checks on, so silent wraparounds show up as caught panics.

Child-process limits are Unix-only; the per-case CPU limit and peak-RSS measurement are
Linux-only. On other systems the cases run in-process without limits (with a warning).
