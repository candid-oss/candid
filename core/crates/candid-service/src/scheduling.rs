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
