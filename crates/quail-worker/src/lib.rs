//! Process isolation for quail: every file is processed in a worker child process, so a
//! crash costs one file, never the run.
//!
//! In Rust a failed allocation aborts the process and a stack overflow kills it;
//! `catch_unwind` (which [`quail_engine`] uses for panics) cannot help with either, nor with
//! a parser stuck in a loop that never checks its deadline. Over millions of untrusted files
//! the only real guarantee is a separate process that can die or be killed:
//!
//! - [`maybe_run_worker`]: called first thing in `main`; turns the process into a worker
//!   when the supervisor started it as one (hidden first argument [`WORKER_ARG`]). The worker
//!   builds its [`Engine`](quail_engine::Engine) once and serves jobs over stdin/stdout.
//! - [`IsolatedRunner`]: the supervisor. Owns `N` workers and mirrors
//!   [`Engine::run`](quail_engine::Engine::run) (same [`RunOptions`](quail_engine::RunOptions):
//!   bounded in-flight files and bytes, ordered output, cancel flag, stop on sink error), so a
//!   CLI can switch to it with a flag. Returns [`IsolatedStats`], a superset of
//!   [`RunStats`](quail_engine::RunStats).
//! - [`WorkerProcess`]: one child, for callers that schedule work themselves.
//!
//! # Guarantees
//!
//! - Every file pulled from the source gets exactly one record. A worker that aborts, runs out
//!   of memory (`RLIMIT_AS`), overflows its stack, exceeds its CPU limit (`RLIMIT_CPU`), exits
//!   or hangs past the wall-clock budget becomes a `Failed` record
//!   (`worker crashed: <reason>`) with the file's size and a format sniffed in the supervisor.
//! - A crashed file is retried once in a fresh worker ([`IsolationOptions::retry_crashed`]);
//!   if it crashes again it is recorded and skipped. The worker is respawned and the run goes
//!   on. Crashes are counted by kind in [`IsolatedStats`].
//! - The supervisor never blocks forever on a worker: every exchange is under a watchdog
//!   that kills the child when its budget runs out.
//!
//! Resource limits apply on Unix only; elsewhere workers run without them but are still
//! isolated and still watched.

mod child;
mod frame;
mod job;
mod runner;
mod sys;
mod worker;

pub use child::{WORKER_ARG, maybe_run_worker};
pub use job::{Job, JobInput, JobSource, PathJobs};
pub use runner::{IsolatedRunner, IsolatedStats, IsolationOptions};
pub use worker::{
    Crash, CrashKind, DEFAULT_RLIMIT_AS, DEFAULT_WORKER_STACK, Processed, StderrMode, WorkerConfig,
    WorkerError, WorkerProcess,
};
