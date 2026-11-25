//! Classify automatic synchronization and queued work for the OS energy scheduler.
//! Interactive request threads retain their existing priority.
pub(crate) fn background() {
    #[cfg(target_os = "macos")]
    {
        // Darwin pthread/qos.h: QOS_CLASS_BACKGROUND = 0x09, relative priority 0.
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        let result = unsafe { pthread_set_qos_class_self_np(0x09, 0) };
        if result != 0 { eprintln!("background scheduling unavailable: {result}"); }
    }
}

/// CPU time this thread has used, in milliseconds (the ledger of background jobs, power v1 §3.6).
pub(crate) fn thread_cpu_ms() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes into the timespec we own.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } != 0 { return 0; }
    t.tv_sec as i64 * 1000 + t.tv_nsec as i64 / 1_000_000
}

/// One-off bulk work the user is waiting for (the library conversion): may use the performance cores, unlike
/// `background`, which macOS keeps on the efficiency cores and throttles.
pub(crate) fn utility() {
    #[cfg(target_os = "macos")]
    {
        // Darwin pthread/qos.h: QOS_CLASS_UTILITY = 0x11.
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        let _ = unsafe { pthread_set_qos_class_self_np(0x11, 0) };
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn background_qos_applies_only_to_the_worker_thread() {
        unsafe extern "C" { fn qos_class_self() -> u32; }
        let caller = unsafe { qos_class_self() };
        let worker = std::thread::spawn(|| {
            super::background();
            unsafe { qos_class_self() }
        }).join().unwrap();
        assert_eq!(worker, 0x09);
        assert_eq!(unsafe { qos_class_self() }, caller);
    }
}
