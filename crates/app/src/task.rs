//! Identity for asynchronous work, independent of the object it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskId(pub u64);

impl TaskId {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}
