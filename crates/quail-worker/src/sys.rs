//! Platform specifics: resource limits in the child, and how a child's death is described.

use std::process::ExitStatus;

/// Applies the child's resource limits to itself. Returns a note per limit that could not be
/// applied (the supervisor logs them; the run goes on without that limit).
///
/// Always disables core dumps on Unix: a corpus of hostile files can crash workers many
/// times, and each core would be as large as the worker's memory.
#[cfg(unix)]
pub(crate) fn apply_child_limits(rlimit_as: Option<u64>) -> Vec<String> {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    let mut notes = Vec::new();
    let mut set = |res: Resource, name: &str, value: u64| {
        let hard = getrlimit(res).map_or(value, |(_, hard)| hard);
        if let Err(e) = setrlimit(res, value.min(hard), hard) {
            notes.push(format!("{name}: {e}"));
        }
    };
    set(Resource::RLIMIT_CORE, "RLIMIT_CORE", 0);
    if let Some(bytes) = rlimit_as {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        ))]
        set(Resource::RLIMIT_AS, "RLIMIT_AS", bytes);
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        {
            let _ = bytes;
            notes.push("RLIMIT_AS: not available on this OS".to_owned());
        }
    }
    notes
}

/// No resource limits outside Unix: the child is still a separate process, so a crash or a
/// hang still costs one file only.
#[cfg(not(unix))]
pub(crate) fn apply_child_limits(rlimit_as: Option<u64>) -> Vec<String> {
    rlimit_as
        .map(|_| vec!["RLIMIT_AS: not available on this OS".to_owned()])
        .unwrap_or_default()
}

/// Sets the soft `RLIMIT_CPU` to the CPU time used so far plus `secs`, so the next job gets
/// `secs` CPU seconds before the kernel sends `SIGXCPU`.
#[cfg(unix)]
pub(crate) fn arm_cpu_limit(secs: u64) {
    use nix::sys::resource::{Resource, UsageWho, getrlimit, getrusage, setrlimit};
    let Ok(usage) = getrusage(UsageWho::RUSAGE_SELF) else {
        return;
    };
    let used = u64::try_from(usage.user_time().tv_sec() + usage.system_time().tv_sec())
        .unwrap_or(0)
        // Partial seconds of both counters.
        + 2;
    let Ok((_, hard)) = getrlimit(Resource::RLIMIT_CPU) else {
        return;
    };
    let _ = setrlimit(
        Resource::RLIMIT_CPU,
        used.saturating_add(secs).min(hard),
        hard,
    );
}

#[cfg(not(unix))]
pub(crate) fn arm_cpu_limit(_secs: u64) {}

/// How the child ended, as far as the exit status tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Death {
    /// Killed by a signal (Unix).
    Signal(i32),
    /// Exited with a code.
    Exit(i32),
    /// Nothing known.
    Unknown,
}

pub(crate) fn death(status: ExitStatus) -> Death {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Death::Signal(sig);
        }
    }
    status.code().map_or(Death::Unknown, Death::Exit)
}

/// Name of a signal number, e.g. `SIGABRT`.
pub(crate) fn signal_name(sig: i32) -> &'static str {
    #[cfg(unix)]
    {
        nix::sys::signal::Signal::try_from(sig).map_or("unknown signal", |s| s.as_str())
    }
    #[cfg(not(unix))]
    {
        let _ = sig;
        "signal"
    }
}

/// Unix signal numbers the classifier cares about.
pub(crate) mod sig {
    #[cfg(unix)]
    pub(crate) const ABRT: i32 = nix::sys::signal::Signal::SIGABRT as i32;
    #[cfg(unix)]
    pub(crate) const XCPU: i32 = nix::sys::signal::Signal::SIGXCPU as i32;
    #[cfg(unix)]
    pub(crate) const KILL: i32 = nix::sys::signal::Signal::SIGKILL as i32;
    #[cfg(unix)]
    pub(crate) const SEGV: i32 = nix::sys::signal::Signal::SIGSEGV as i32;
    #[cfg(unix)]
    pub(crate) const BUS: i32 = nix::sys::signal::Signal::SIGBUS as i32;
    #[cfg(not(unix))]
    pub(crate) const ABRT: i32 = -1;
    #[cfg(not(unix))]
    pub(crate) const XCPU: i32 = -2;
    #[cfg(not(unix))]
    pub(crate) const KILL: i32 = -3;
    #[cfg(not(unix))]
    pub(crate) const SEGV: i32 = -4;
    #[cfg(not(unix))]
    pub(crate) const BUS: i32 = -5;
}

/// Windows exit codes of a crashed process (NTSTATUS values as `i32`).
pub(crate) mod nt {
    /// `STATUS_STACK_OVERFLOW`.
    #[allow(clippy::cast_possible_wrap)]
    pub(crate) const STACK_OVERFLOW: i32 = 0xC000_00FD_u32 as i32;
    /// `STATUS_STACK_BUFFER_OVERRUN`: `__fastfail`, which `std::process::abort` uses.
    #[allow(clippy::cast_possible_wrap)]
    pub(crate) const FAST_FAIL: i32 = 0xC000_0409_u32 as i32;
    /// `STATUS_ACCESS_VIOLATION`.
    #[allow(clippy::cast_possible_wrap)]
    pub(crate) const ACCESS_VIOLATION: i32 = 0xC000_0005_u32 as i32;
}
