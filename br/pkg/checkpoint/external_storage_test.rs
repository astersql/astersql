// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::checkpoint::{checkpointStorage, flushPath};
use crate::external_storage::{
    newExternalCheckpointStorage, set_failed_after_checkpoint_updates_lock_for_test,
};
use crate::stubs::{Context, Error, GlobalTimer, MemStorage, Result, Storage};

struct EventuallySuccessfulTimer {
    calls: AtomicUsize,
    failures: usize,
}

impl GlobalTimer for EventuallySuccessfulTimer {
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.failures {
            Err(Error::new("transient PD error"))
        } else {
            Ok((100, 7))
        }
    }
}

fn test_flush_path() -> flushPath {
    flushPath {
        CheckpointDataDir: "checkpoint/data".to_string(),
        CheckpointChecksumDir: "checkpoint/checksum".to_string(),
        CheckpointLockPath: "checkpoint/lock".to_string(),
    }
}

#[test]
fn get_ts_uses_go_aggressive_pd_attempt_count() {
    let ctx = Context::Background();
    let storage: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let timer = Arc::new(EventuallySuccessfulTimer {
        calls: AtomicUsize::new(0),
        failures: 8,
    });
    let timer_for_storage: Arc<dyn GlobalTimer> = timer.clone();

    newExternalCheckpointStorage(&ctx, storage, Some(timer_for_storage), test_flush_path())
        .unwrap();
    assert_eq!(timer.calls.load(Ordering::SeqCst), 9);
}

#[test]
fn update_lock_preserves_go_post_write_failpoint() {
    let ctx = Context::Background();
    let storage: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let timer: Arc<dyn GlobalTimer> = Arc::new(EventuallySuccessfulTimer {
        calls: AtomicUsize::new(0),
        failures: 0,
    });
    let checkpoint =
        newExternalCheckpointStorage(&ctx, storage, Some(timer), test_flush_path()).unwrap();

    set_failed_after_checkpoint_updates_lock_for_test(true);
    let result = checkpoint.updateLock(&ctx);
    set_failed_after_checkpoint_updates_lock_for_test(false);

    assert_eq!(
        result.unwrap_err().to_string(),
        "failpoint: failed after checkpoint updates lock"
    );
}
