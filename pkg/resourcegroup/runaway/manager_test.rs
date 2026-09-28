// Copyright 2026 AsterSQL.

use std::sync::Arc;

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

fn manager() -> Manager {
    Manager::NewRunawayManager(
        Arc::new(EmptyCatalog),
        "server-1",
        Arc::new(NoopExecutor),
        Arc::new(AllSystemTables),
    )
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
