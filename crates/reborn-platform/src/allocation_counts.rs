//! Benchmark-only Rust allocation counters for the calling thread.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct Counts {
    calls: u64,
    allocated_bytes: u64,
}
thread_local! {
    static COUNTS: Cell<Counts> = const { Cell::new(Counts { calls: 0, allocated_bytes: 0 }) };
}
fn record(bytes: usize) {
    let _ = COUNTS.try_with(|v| {
        let mut count = v.get();
        count.calls += 1;
        count.allocated_bytes += bytes as u64;
        v.set(count);
    });
}
pub struct Counting;
// SAFETY: every allocation operation delegates the original pointer/layout to
// System. Constant thread-local counters allocate nothing and retain no pointer.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            record(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            record(layout.size());
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, layout, size) };
        if !p.is_null() {
            record(size);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) }
    }
}
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Counts) {
    let before = COUNTS.with(Cell::get);
    let result = f();
    let after = COUNTS.with(Cell::get);
    (
        result,
        Counts {
            calls: after.calls - before.calls,
            allocated_bytes: after.allocated_bytes - before.allocated_bytes,
        },
    )
}
pub fn summary(samples: &[Counts]) -> serde_json::Value {
    let n = samples.len().max(1) as f64;
    serde_json::json!({"samples":samples.len(),
        "mean_calls":samples.iter().map(|v|v.calls as f64).sum::<f64>()/n,
        "mean_allocated_bytes":samples.iter().map(|v|v.allocated_bytes as f64).sum::<f64>()/n,
        "max_calls":samples.iter().map(|v|v.calls).max(),
        "max_allocated_bytes":samples.iter().map(|v|v.allocated_bytes).max(),
        "scope":"calling-thread Rust allocations and reallocations; excludes C/GL driver allocations; bytes are cumulative, not retained heap"})
}
