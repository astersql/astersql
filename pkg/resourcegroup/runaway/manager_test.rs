// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::manager::{Manager, ManualSource};
use crate::record::QuarantineRecord;
use crate::syncer::AllSystemTables;
use crate::{NoopExecutor, ResourceGroup, ResourceGroupCatalog, Result, RunawayAction};

#[derive(Default)]
struct EmptyCatalog;

impl ResourceGroupCatalog for EmptyCatalog {
    fn GetResourceGroup(&self, _name: &str) -> Result<Option<ResourceGroup>> {
        Ok(None)
    }
}

struct ConfiguredCatalog;

impl ResourceGroupCatalog for ConfiguredCatalog {
    fn GetResourceGroup(&self, name: &str) -> Result<Option<ResourceGroup>> {
        Ok(Some(ResourceGroup {
            name: name.into(),
            runaway_settings: Some(Default::default()),
        }))
    }
}

fn manager() -> Manager {
    Manager::NewRunawayManager(
        Arc::new(EmptyCatalog),
        "server-1",
        Arc::new(NoopExecutor),
        Arc::new(AllSystemTables),
    )
}

#[test]
fn diagnostic_mode_suppresses_runaway_manager_work() {
    struct RestoreInTest(bool);
    impl Drop for RestoreInTest {
        fn drop(&mut self) {
            astersql_util_intest::InTest.store(self.0, Ordering::SeqCst);
        }
    }

    struct RestoreMode<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for RestoreMode<F> {
        fn drop(&mut self) {
            if let Some(restore) = self.0.take() {
                restore();
            }
        }
    }

    let previous = astersql_util_intest::InTest.swap(true, Ordering::SeqCst);
    let _restore_in_test = RestoreInTest(previous);
    let _restore_mode = RestoreMode(Some(astersql_config::diagnosticmode::SetForTest(false)));

    let normal = Manager::NewRunawayManager(
        Arc::new(ConfiguredCatalog),
        "server-1",
        Arc::new(NoopExecutor),
        Arc::new(AllSystemTables),
    );
    assert!(normal.inNormalMode());
    assert!(
        normal
            .DeriveChecker(
                "default",
                "select 1".into(),
                "sql-digest".into(),
                "plan-digest".into(),
                0,
            )
            .is_some()
    );

    let _restore_diagnostic = RestoreMode(Some(astersql_config::diagnosticmode::SetForTest(true)));
    let diagnostic = manager();
    assert!(!diagnostic.inNormalMode());
    assert!(
        normal
            .DeriveChecker(
                "default",
                "select 1".into(),
                "sql-digest".into(),
                "plan-digest".into(),
                0,
            )
            .is_none()
    );
    diagnostic.Stop();
    assert!(diagnostic.stopped());
}

fn watch(id: i64, source: &str, cause: &str) -> QuarantineRecord {
    QuarantineRecord {
        ID: id,
        ResourceGroupName: "rg".into(),
        WatchText: "digest".into(),
        Source: source.into(),
        Action: RunawayAction::Kill,
        ExceedCause: cause.into(),
        ..Default::default()
    }
}

#[test]
fn duplicate_and_manual_watch_replacement_match_go() {
    let manager = manager();
    manager.addWatchList(watch(7, "server-1", "original"), false);

    // Go ignores a repeated scan of the same persisted row, even if the incoming
    // object differs in non-key fields.
    manager.addWatchList(watch(7, "server-2", "duplicate"), false);
    assert_eq!(manager.GetWatchList()[0].ExceedCause, "original");

    // The manual force path also returns early for an identical ID.
    manager.addWatchList(watch(7, ManualSource, "same-id manual"), true);
    assert_eq!(manager.GetWatchList()[0].ExceedCause, "original");

    // Replacing a different persisted ID evicts the old row, which Go sends to
    // staleQuarantineRecord so its system-table row is cleaned up.
    manager.addWatchList(watch(8, ManualSource, "replacement"), true);
    assert_eq!(manager.GetWatchList()[0].ID, 8);
    let stale = manager.drainStaleRecords();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].ID, 7);
}
