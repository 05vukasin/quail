# quail-worker

Process isolation for quail. Every file is processed in a worker child process, so a crash
costs one file and the run keeps going.

In Rust a failed allocation aborts the process and a stack overflow kills it. `catch_unwind`,
which `quail-engine` uses for panics, can't stop either one, and it can't stop a parser that
loops without checking its deadline. Separate processes with resource limits and a watchdog
are the only real guarantee when the input is millions of untrusted files on a 4 GB machine.

No fact keys, no analyzers: this crate runs the engine, it doesn't add to it.

## Use

```rust
fn engine() -> quail_engine::Engine { /* register extractors and analyzers */ }

fn main() {
    quail_worker::maybe_run_worker(engine); // first thing; returns unless we are a worker
    // ... parse args, build source and sink ...
    let mut runner = quail_worker::IsolatedRunner::new(Default::default());
    let stats = runner.run(&mut source, &mut sink, &quail_engine::RunOptions::default())?;
}
```

- `maybe_run_worker(build_engine)`: the supervisor re-runs the current executable with the
  hidden first argument `__quail-worker`. In that mode the process applies its limits, builds
  the engine once (models load once per child), serves jobs over stdin/stdout until stdin
  closes, then exits. On Unix, stdout is pointed at stderr so a stray `println!` can't corrupt
  the protocol.
- `IsolatedRunner::run(source, sink, &RunOptions) -> Result<IsolatedStats, WorkerError>`:
  works like `Engine::run`. It uses the same `RunOptions`: `threads` workers, bounded
  `max_in_flight` and `max_in_flight_bytes`, `ordered`, `cancel` and `stop_on_sink_error`.
  `on_artifacts` gets the `document` artifact, sent back from the worker. `Err` means only
  that the first workers could not start. Use `warm_up(n)` to find that out early.
- `run_jobs(&mut dyn JobSource, ...)` with `PathJobs`: the worker reads the file itself, so
  file bytes never pass through the supervisor or the pipe.
- `IsolatedRunner::process(&job, want_document)` handles one file, with the same retry
  handling. `WorkerProcess` is a single child, for callers that do their own scheduling.

## Guarantees

| the worker...                            | record                                              | stats key        |
|------------------------------------------|-----------------------------------------------------|------------------|
| panics in a step                         | `Partial`, in process (engine's `catch_unwind`)     | -                |
| aborts                                   | `Failed: worker crashed: aborted (SIGABRT (6))`      | `abort`          |
| allocates past `RLIMIT_AS`               | `Failed: worker crashed: out of memory: ...`         | `oom`            |
| overflows its stack                      | `Failed: worker crashed: stack overflow ...`         | `stack_overflow` |
| exceeds `rlimit_cpu` for one file         | `Failed: worker crashed: CPU time limit exceeded`    | `cpu_limit`      |
| hangs past the wall-clock budget          | `Failed: worker crashed: timed out ... watchdog`     | `timeout`        |
| exits, is killed (kernel OOM killer)      | `Failed: worker crashed: exited with code N` / `killed ...` | `exit` / `killed` / `signal` |
| breaks the protocol                       | `Failed: worker crashed: protocol error: ...`        | `protocol`       |

- Every file pulled from the source gets exactly one record. A crash record has the file's
  size and a format sniffed in the supervisor (`quail_engine::sniff`; path jobs read up to
  16 MiB, or the first 64 KiB of a larger file). The reason includes the stderr line that
  explains the death best, for example `memory allocation of 16777216 bytes failed`.
- A crashed file is retried once in a fresh worker (`retry_crashed`, on by default). If it
  succeeds, the record carries a warning about the first crash. If it crashes again, it is
  recorded as `Failed (twice; skipped)` and counted in `poisoned`. Timeouts are not retried
  unless `retry_timeouts` is set, because a retry costs the whole budget again.
- Dead workers are respawned when their seat is next used (`respawns`).
- No deadlocks: every exchange with a child runs under a per-child watchdog. The default
  budget is `kill_after = 2 × Limits::timeout + grace (5 s)`, so the engine's cooperative
  deadline gets the first chance. Startup has its own `startup_timeout` (60 s).
- Core dumps are disabled in the workers (`RLIMIT_CORE = 0`). Crashing on a corpus of
  hostile files would otherwise fill the disk.

## Limits (`WorkerConfig`)

- `rlimit_as`: address space per child, 1 GiB by default. This covers everything the child
  maps: the binary, a 32 MiB processing stack (`stack_size`), malloc arenas, models, and the
  file plus its document. Size it to the largest model plus about 4× the largest file you
  accept. Budget: `N × rlimit_as` plus the supervisor's in-flight bytes.
- `rlimit_cpu`: CPU seconds per file, re-armed before each job. Off by default.
- `stderr`: `Forward` (the default) or `Discard`. The last 8 KiB are always kept to explain
  a crash.

## Cost

Measured with `cargo bench -p quail-worker --profile profiling` on 2 KiB text files and a
trivial engine (extractor plus 4 analyzers, 10 facts). The machine was shared and busy, so
the numbers are indicative:

| N | in-process      | isolated, bytes | overhead per worker | + document back |
|---|-----------------|-----------------|---------------------|-----------------|
| 1 | 35.6 k files/s  | 16.1 k files/s  | 34 µs/file          | 57 µs/file      |
| 4 | 76.8 k files/s  | 37.7 k files/s  | 54 µs/file          | 48 µs/file      |
| 8 | 165 k files/s   | 61.9 k files/s  | 81 µs/file          | 128 µs/file     |

Paths read by the worker (N=4): 28.6 k files/s, against 74.2 k in process.

About 15 µs of this is the JSON round trip of the record (1.7 KB). The rest is two pipe
round trips and the thread wake-ups. Real PDF and DOCX extraction costs milliseconds per
file, so isolation adds well under 5% there, and it buys survival of aborts, OOMs, stack
overflows and hangs. For tiny files, or a trusted corpus, run in process. A crash costs one
respawn: process start plus `build_engine`. If loading models takes seconds, a corpus that
crashes on most files pays that each time.

JSON is the record encoding because `FileRecord` uses internally tagged enums
(`#[serde(tag = ...)]`), which non-self-describing formats like postcard and bincode can't
read back. A binary encoding would need a core change.

## Platform notes

- Linux: all limits are enforced. This is the setup the failure-matrix tests cover.
- macOS: `RLIMIT_CPU` and `RLIMIT_CORE` work. `RLIMIT_AS` is accepted but not reliably
  enforced, so a runaway allocation may grow until the kernel kills the child (`killed`).
- Windows and others: no rlimits, and stdout is not redirected. Workers are still separate
  processes under the watchdog. Crash codes are mapped: fail-fast means aborted, and
  `STATUS_STACK_OVERFLOW` means stack overflow. Job Objects for memory caps are future work.
- `quail-worker-demo FILE...` is a tiny demo binary. A file that contains `QUAIL-DEMO-ABORT`
  aborts its worker, and you can watch the run carry on.
