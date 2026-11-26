//! Per-thread disk priority of the runtime's own bookkeeping.
//!
//! The shipped user units start the service with the lowest best-effort disk priority
//! (`IOSchedulingPriority=7`) so the processes it starts (engines, builds, renders)
//! yield the disk to the desktop session. Linux keeps that priority per thread and
//! every new thread and child inherits it, so without a correction the store
//! connections and the owner's project reads ran at that level too: on a busy
//! rotational disk under BFQ a read of one page could wait minutes behind any
//! default-priority process, and the owner's project read missed its deadline.
//!
//! A thread that serves the store or an owner read raises itself to the session's
//! default (`IOPRIO_CLASS_NONE`, the level every ordinary session process has).
//! Raising within the best-effort and idle classes needs no privilege. Where the
//! syscall is unavailable or refused the thread keeps its inherited level: nothing
//! is reported as changed that did not change.

/// `IOPRIO_WHO_PROCESS` with id 0 names the calling thread.
#[cfg(target_os = "linux")]
const WHO_THREAD: libc::c_int = 1;
const CLASS_SHIFT: u32 = 13;
const CLASS_NONE: i32 = 0;
const CLASS_BEST_EFFORT: i32 = 2;
const CLASS_IDLE: i32 = 3;
/// The best-effort level a default-priority process (nice 0) is served at.
const DEFAULT_LEVEL: i32 = 4;

/// The calling thread's raw I/O priority, or `None` when it cannot be read.
pub fn current() -> Option<i32> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: ioprio_get takes two integers and touches no memory.
        let value = unsafe { libc::syscall(libc::SYS_ioprio_get, WHO_THREAD, 0) };
        (value >= 0).then_some(value as i32)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn set(value: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: ioprio_set takes three integers and touches no memory.
        unsafe { libc::syscall(libc::SYS_ioprio_set, WHO_THREAD, 0, value) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = value;
        false
    }
}

/// True when `value` is served below a default-priority session process.
pub fn below_default(value: i32) -> bool {
    let class = value >> CLASS_SHIFT;
    let level = value & ((1 << CLASS_SHIFT) - 1);
    class == CLASS_IDLE || (class == CLASS_BEST_EFFORT && level > DEFAULT_LEVEL)
}

/// Raise the calling thread to the default disk priority for the rest of its life.
/// For a dedicated thread (a store connection lane) that starts no other process.
/// Returns whether the thread now runs at the default level because of this call.
pub fn raise_thread() -> bool {
    match current() {
        Some(value) if below_default(value) => set(CLASS_NONE),
        _ => false,
    }
}

/// The default disk priority for one owner read on a pooled or short-lived thread.
/// Dropping it restores the level the thread had, so a pooled thread later used to
/// start a task process never hands that process the raised level.
#[must_use = "the raised priority lasts only while the guard is held"]
pub struct Raised {
    previous: Option<i32>,
}

/// Raise the calling thread until the returned guard is dropped (see [`Raised`]).
pub fn raised() -> Raised {
    let previous = current().filter(|&value| below_default(value) && set(CLASS_NONE));
    Raised { previous }
}

impl Raised {
    /// Whether this guard changed the thread's level (and restores it on drop).
    pub fn changed(&self) -> bool {
        self.previous.is_some()
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            let _ = set(previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best_effort(level: i32) -> i32 {
        (CLASS_BEST_EFFORT << CLASS_SHIFT) | level
    }

    #[test]
    fn only_levels_below_the_session_default_are_raised() {
        assert!(below_default(best_effort(7)));
        assert!(below_default(best_effort(5)));
        assert!(below_default(CLASS_IDLE << CLASS_SHIFT));
        assert!(!below_default(best_effort(4)));
        assert!(!below_default(best_effort(0)));
        assert!(!below_default(CLASS_NONE));
        assert!(!below_default(1 << CLASS_SHIFT));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_lowered_thread_is_raised_and_restored() {
        // A fresh thread so the test runner's own threads keep their level.
        std::thread::spawn(|| {
            assert!(set(best_effort(7)), "lowering a thread's own level needs no privilege");
            {
                let guard = raised();
                assert!(guard.changed());
                assert_eq!(current(), Some(CLASS_NONE));
            }
            assert_eq!(current(), Some(best_effort(7)), "the guard restores the inherited level");
            assert!(raise_thread());
            assert_eq!(current(), Some(CLASS_NONE));
            assert!(!raise_thread(), "an already default thread is left alone");
            let guard = raised();
            assert!(!guard.changed());
            drop(guard);
            assert_eq!(current(), Some(CLASS_NONE));
        })
        .join()
        .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_thread_started_by_a_lowered_thread_inherits_its_level() {
        // The reason a pooled thread must restore its level: children inherit it.
        std::thread::spawn(|| {
            assert!(set(best_effort(7)));
            let inherited = std::thread::spawn(current).join().unwrap();
            assert_eq!(inherited, Some(best_effort(7)));
            let _guard = raised();
            let inherited = std::thread::spawn(current).join().unwrap();
            assert_eq!(inherited, Some(CLASS_NONE));
        })
        .join()
        .unwrap();
    }
}
