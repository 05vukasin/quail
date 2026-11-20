//! Panic containment: run a step under `catch_unwind` and, optionally, keep the default
//! panic hook quiet while doing so.

use std::any::Any;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use quail_core::Error;

thread_local! {
    /// True while this thread runs a contained step whose panics should not print.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

static HOOK: Once = Once::new();

/// Installs, once per process, a panic hook that stays silent for panics raised inside a
/// step the engine contains with [`contain`] (when the engine was built with
/// `quiet_panics(true)`), and defers to the previous hook for every other panic.
///
/// Hostile files can make third-party parsers panic thousands of times in a run; the panic
/// is already recorded in the file's status, so printing it to stderr is just noise.
pub fn install_quiet_panic_hook() {
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !QUIET.with(Cell::get) {
                previous(info);
            }
        }));
    });
}

/// Runs `f`, turning a panic into [`Error::Panic`] with the panic message.
///
/// `quiet` marks this thread so the hook from [`install_quiet_panic_hook`] stays silent.
pub(crate) fn contain<T>(quiet: bool, f: impl FnOnce() -> T) -> Result<T, Error> {
    let was = QUIET.with(|q| q.replace(quiet));
    let r = panic::catch_unwind(AssertUnwindSafe(f));
    QUIET.with(|q| q.set(was));
    r.map_err(|payload| Error::Panic(payload_message(payload.as_ref())))
}

/// The message of a panic payload (`&str` or `String`), or a placeholder.
pub(crate) fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        install_quiet_panic_hook();
        let e = contain(true, || panic!("boom {}", 7)).unwrap_err();
        assert!(matches!(e, Error::Panic(m) if m == "boom 7"));
        let e = contain(true, || panic!("static")).unwrap_err();
        assert!(matches!(e, Error::Panic(m) if m == "static"));
        let e = contain(true, || std::panic::panic_any(42_u8)).unwrap_err();
        assert!(matches!(e, Error::Panic(m) if m == "non-string panic payload"));
        assert_eq!(contain(true, || 5).unwrap(), 5);
    }
}
