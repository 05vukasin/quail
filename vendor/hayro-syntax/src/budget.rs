//! quail: per-file decode budget (this whole module is a quail addition).
//!
//! Every stream decode (object streams, content streams, fonts, images...) takes its output
//! size from one budget shared by everything decoded out of a [`PdfData`](crate::PdfData).
//! Decoders stop as soon as they would exceed what is left, and image decoders refuse
//! dimensions above the pixel cap before they allocate, so a hostile file cannot make the
//! process allocate more than the budget (an allocation failure aborts the process; it cannot
//! be caught).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Bytes decoded per stream when no budget is attached (e.g. inline images, which carry no
/// document context): a fixed ceiling so that budget-less decoding is still bounded.
pub const DEFAULT_MAX_STREAM_BYTES: u64 = 64 << 20;
/// Pixels per image when no budget is attached.
pub const DEFAULT_MAX_IMAGE_PIXELS: u64 = 64 << 20;

/// What decoding one file may still produce.
#[derive(Debug)]
pub struct DecodeBudget {
    remaining: AtomicU64,
    max_pixels: AtomicU64,
    limit_hit: AtomicBool,
}

impl DecodeBudget {
    /// A budget of `max_bytes` decoded bytes in total and `max_pixels` per image.
    pub fn new(max_bytes: u64, max_pixels: u64) -> Self {
        Self {
            remaining: AtomicU64::new(max_bytes),
            max_pixels: AtomicU64::new(max_pixels),
            limit_hit: AtomicBool::new(false),
        }
    }

    /// Starts over with new limits (e.g. one budget per render call).
    pub fn reset(&self, max_bytes: u64, max_pixels: u64) {
        self.remaining.store(max_bytes, Ordering::Relaxed);
        self.max_pixels.store(max_pixels, Ordering::Relaxed);
        self.limit_hit.store(false, Ordering::Relaxed);
    }

    /// Whether some decode was refused because it would exceed a limit (since the last
    /// [`reset`](Self::reset)).
    pub fn limit_hit(&self) -> bool {
        self.limit_hit.load(Ordering::Relaxed)
    }

    /// Returns whether a limit was hit and clears the flag.
    pub fn take_limit_hit(&self) -> bool {
        self.limit_hit.swap(false, Ordering::Relaxed)
    }

    /// Records that a decode was refused for exceeding a limit.
    pub fn mark_limit_hit(&self) {
        self.limit_hit.store(true, Ordering::Relaxed);
    }

    /// Decoded bytes still allowed.
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }

    /// Largest image, in pixels, a decoder may produce.
    pub fn max_pixels(&self) -> u64 {
        self.max_pixels.load(Ordering::Relaxed)
    }

    /// Takes `n` bytes from the budget; `false` (and nothing taken) if fewer are left.
    pub fn charge(&self, n: u64) -> bool {
        let ok = self
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |r| r.checked_sub(n))
            .is_ok();
        if !ok {
            self.mark_limit_hit();
        }
        ok
    }
}

/// Growth step below which buffers are not grown (64 KiB).
const MIN_GROWTH: usize = 64 << 10;

/// Grows `v` so that `extra` more bytes fit. The capacity grows by half at a time, but never
/// past `cap + extra`, so a buffer capped at `cap` never doubles to `2 × cap`. The allocation
/// goes through `try_reserve_exact`, so a failure is reported (`false`), never an abort.
pub(crate) fn reserve_capped(v: &mut Vec<u8>, extra: usize, cap: usize) -> bool {
    let need = v.len().saturating_add(extra);
    if need <= v.capacity() {
        return true;
    }
    let ceiling = cap.saturating_add(extra).max(need);
    let target = need
        .max(v.capacity().saturating_add(v.capacity() / 2))
        .max(MIN_GROWTH)
        .min(ceiling);
    v.try_reserve_exact(target - v.len()).is_ok()
}

/// Reads `r` to the end in 64 KiB chunks into a buffer grown by [`reserve_capped`]. Stops just
/// past `cap` (the caller sees `len() > cap` and refuses). `None` when an allocation fails.
#[cfg(feature = "std")]
pub(crate) fn read_capped<R: std::io::Read>(mut r: R, cap: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64 << 10];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return Some(out),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let chunk = buf.get(..n)?;
        if !reserve_capped(&mut out, n, cap) {
            return None;
        }
        out.extend_from_slice(chunk);
        if out.len() > cap {
            return Some(out);
        }
    }
}

/// The limits one decode call works under.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecodeLimits<'a> {
    /// Largest output of any single filter layer.
    pub max_out: u64,
    /// Largest image in pixels.
    pub max_pixels: u64,
    budget: Option<&'a DecodeBudget>,
}

impl<'a> DecodeLimits<'a> {
    pub(crate) fn from_budget(b: Option<&'a DecodeBudget>) -> Self {
        match b {
            Some(b) => Self {
                max_out: b.remaining(),
                max_pixels: b.max_pixels(),
                budget: Some(b),
            },
            None => Self {
                max_out: DEFAULT_MAX_STREAM_BYTES,
                max_pixels: DEFAULT_MAX_IMAGE_PIXELS,
                budget: None,
            },
        }
    }

    /// Records that a decode was refused for exceeding a limit.
    pub(crate) fn refuse(&self) {
        if let Some(b) = self.budget {
            b.mark_limit_hit();
        }
    }

    /// `w × h × components` fits both the pixel cap and the output cap.
    pub(crate) fn image_fits(&self, width: u32, height: u32, components: u64) -> bool {
        let px = u64::from(width).saturating_mul(u64::from(height));
        px <= self.max_pixels && px.saturating_mul(components.max(1)) <= self.max_out
    }
}
