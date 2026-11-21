//! quail: render limits (this whole module is a quail addition).
//!
//! hayro's renderer allocates pixmaps sized from pattern cells, soft-mask canvases and image
//! dictionaries, and it cannot be preempted. A [`RenderLimits`] passed to
//! [`render_limited`](crate::render_limited) bounds one render:
//! - a byte budget charged, *before* allocating, for every decoded image (8 bytes per declared
//!   pixel: samples plus the RGBA copy), every child render context (pattern tiles, stencil
//!   paints, 8 bytes per pixel) and every soft-mask canvas;
//! - a deadline, checked every 256 device callbacks and every tile copy;
//! - a cap on device callbacks.
//!
//! When a limit is hit the render unwinds with a [`RenderAbort`] payload through
//! `resume_unwind` (no panic hook); the caller catches it and reports the limit.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Why a render was cut short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderAbort {
    /// The deadline passed.
    Deadline,
    /// The byte budget ran out.
    Memory,
    /// Too many device callbacks.
    Calls,
}

/// Limits for one render.
#[derive(Debug)]
pub struct RenderLimits {
    deadline: Option<Instant>,
    bytes_left: AtomicU64,
    calls: AtomicU64,
    max_calls: u64,
}

impl RenderLimits {
    /// A render that must end by `deadline`, may allocate `max_bytes` for images and child
    /// pixmaps, and may make `max_calls` device callbacks.
    pub fn new(deadline: Option<Instant>, max_bytes: u64, max_calls: u64) -> Self {
        Self {
            deadline,
            bytes_left: AtomicU64::new(max_bytes),
            calls: AtomicU64::new(0),
            max_calls,
        }
    }

    /// Bytes of the budget still unused.
    pub fn bytes_left(&self) -> u64 {
        self.bytes_left.load(Ordering::Relaxed)
    }

    pub(crate) fn tick(&self) {
        let n = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if n > self.max_calls {
            abort(RenderAbort::Calls);
        }
        if n.is_multiple_of(256)
            && let Some(d) = self.deadline
            && Instant::now() >= d
        {
            abort(RenderAbort::Deadline);
        }
    }

    pub(crate) fn charge(&self, bytes: u64) {
        if self
            .bytes_left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |r| r.checked_sub(bytes))
            .is_err()
        {
            abort(RenderAbort::Memory);
        }
    }
}

fn abort(kind: RenderAbort) -> ! {
    std::panic::resume_unwind(Box::new(kind))
}
