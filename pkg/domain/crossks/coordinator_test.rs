// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use crate::{InternalSession, JobMdl, new_schema_coordinator};

struct BlockingSession {
    entered: mpsc::SyncSender<()>,
    release: Arc<Barrier>,
}

impl InternalSession for BlockingSession {
    fn id(&self) -> u64 {
        1
    }

    fn remove_lock_ddl_jobs(&self, _jobs: &HashMap<i64, JobMdl>, _print_log: bool) {
        self.entered
            .send(())
            .expect("test must still wait for the callback");
        self.release.wait();
    }
}

#[test]
fn deleting_a_session_waits_for_an_in_progress_mdl_check() {
    let coordinator = Arc::new(new_schema_coordinator());
    let (entered_tx, entered_rx) = mpsc::sync_channel(0);
    let release = Arc::new(Barrier::new(2));
    coordinator.store_internal_session(Arc::new(BlockingSession {
        entered: entered_tx,
        release: Arc::clone(&release),
    }));

    let checking_coordinator = Arc::clone(&coordinator);
    let checker = std::thread::spawn(move || {
        checking_coordinator.check_old_running_transaction(&HashMap::new());
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("MDL callback must start");

    let (deleted_tx, deleted_rx) = mpsc::channel();
    let deleting_coordinator = Arc::clone(&coordinator);
    let deleter = std::thread::spawn(move || {
        deleting_coordinator.delete_internal_session(1);
        deleted_tx
            .send(())
            .expect("test must still observe deletion completion");
    });

    let deletion_finished_during_callback =
        match deleted_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => panic!("deletion worker disconnected"),
        };
    release.wait();
    checker.join().expect("MDL check worker must not panic");
    deleter.join().expect("deletion worker must not panic");

    assert!(
        !deletion_finished_during_callback,
        "Go keeps the coordinator read lock through each Session callback"
    );
    assert_eq!(coordinator.internal_session_count(), 0);
}
