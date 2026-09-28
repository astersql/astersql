// Copyright 2026 AsterSQL.

//! Go-equivalent lifecycle tests for `br/pkg/task/backup_raw.go`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backup_raw::{RawKvConfig, RunBackupRaw};
use crate::stubs::{
    Error, MemBackupClient, MemGlue, Mgr, RestoreSchedulers, Result, TLSConfigInner,
};

struct FailingMgr {
    restored: Arc<AtomicBool>,
}

impl Mgr for FailingMgr {
    fn Close(&self) {}

    fn GetClusterVersion(&self) -> Result<String> {
        Err(Error::new("cluster version unavailable"))
    }

    fn GetRegionCount(&self, _start: &[u8], _end: &[u8]) -> Result<usize> {
        Ok(1)
    }

    fn RemoveSchedulers(&self) -> Result<RestoreSchedulers> {
        let restored = Arc::clone(&self.restored);
        Ok(Box::new(move || {
            restored.store(true, Ordering::SeqCst);
            Ok(())
        }))
    }

    fn GetTLSConfig(&self) -> Option<TLSConfigInner> {
        None
    }

    fn UpdatePDScheduleConfig(&self) -> Result<()> {
        Ok(())
    }
}

/// Go installs the scheduler restore callback with `defer`, so every later error
/// must restore the removed schedulers before returning.
#[test]
fn test_run_backup_raw_restores_schedulers_on_error() {
    let restored = Arc::new(AtomicBool::new(false));
    let mgr = Arc::new(FailingMgr {
        restored: Arc::clone(&restored),
    });
    let mut cfg = RawKvConfig {
        RemoveSchedulers: true,
        ..Default::default()
    };

    let err = RunBackupRaw(
        &MemGlue::default(),
        "Raw Backup",
        &mut cfg,
        mgr,
        &MemBackupClient::default(),
    )
    .unwrap_err();

    assert!(err.msg.contains("cluster version unavailable"));
    assert!(
        restored.load(Ordering::SeqCst),
        "scheduler restore callback must run on error"
    );
}
