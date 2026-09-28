// Copyright 2026 AsterSQL.

use astersql_util_intest::{EnableInternalCheck, InTest};
use std::sync::atomic::Ordering;

// An integration test exercises the public library without its unit-test cfg.
#[test]
fn test_flag_can_be_overridden_observed_and_restored() {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            InTest.store(self.0, Ordering::SeqCst);
        }
    }

    let initial = InTest.load(Ordering::SeqCst);
    assert_eq!(initial, cfg!(feature = "intest"));
    let internal_checks = EnableInternalCheck.load(Ordering::SeqCst);
    {
        let _restore = Restore(initial);
        assert_eq!(InTest.swap(!initial, Ordering::SeqCst), initial);
        assert_eq!(
            std::thread::spawn(|| InTest.load(Ordering::SeqCst))
                .join()
                .unwrap(),
            !initial
        );
        InTest.store(initial, Ordering::SeqCst);
        assert_eq!(InTest.load(Ordering::SeqCst), initial);
        InTest.store(!initial, Ordering::SeqCst);
    }
    assert_eq!(InTest.load(Ordering::SeqCst), initial);
    // Go initializes internal checks once; overriding InTest does not reset them.
    assert_eq!(EnableInternalCheck.load(Ordering::SeqCst), internal_checks);
}
