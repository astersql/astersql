// Copyright 2026 AsterSQL.

use super::mocksessionmanager::{MockSessionManager, ProcessInfo, SessionSnapshot, TxnInfo};
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

fn process(id: u64) -> ProcessInfo {
    ProcessInfo {
        id,
        user: "root".to_owned(),
        database: "test".to_owned(),
        command: "Query".to_owned(),
        started_at: SystemTime::now(),
        killed: false,
    }
}

#[test]
fn kill_is_a_noop_like_the_go_mock() {
    let manager = MockSessionManager::default();
    manager.StoreProcessInfo(process(7));

    assert!(!manager.Kill(7));
    assert!(!manager.GetProcessInfo(7).unwrap().killed);
}

fn txn(connection_id: u64, start_ts: u64, has_process_info: bool) -> TxnInfo {
    TxnInfo {
        connection_id,
        start_ts,
        current_sql_digest: Some("digest".to_owned()),
        has_process_info,
    }
}

#[test]
fn explicit_processes_and_transactions_override_connection_fallbacks() {
    let manager = MockSessionManager::default();
    manager.StoreConnection(
        8,
        SessionSnapshot {
            process_info: Some(process(8)),
            txn_info: Some(txn(8, 80, true)),
            connection_id: 8,
            ..SessionSnapshot::default()
        },
    );
    assert_eq!(
        manager
            .ShowProcessList()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [8]
    );
    assert_eq!(manager.ShowTxnList(), [txn(8, 80, true)]);

    manager.StoreProcessInfo(process(7));
    manager.SetTxnInfo(txn(7, 70, true));
    assert_eq!(
        manager
            .ShowProcessList()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [7]
    );
    assert_eq!(manager.ShowTxnList(), [txn(7, 70, true)]);
}

#[test]
fn connection_txn_without_process_info_is_filtered_like_go() {
    let manager = MockSessionManager::default();
    manager.StoreConnection(
        1,
        SessionSnapshot {
            txn_info: Some(txn(1, 10, false)),
            ..SessionSnapshot::default()
        },
    );
    manager.StoreConnection(
        2,
        SessionSnapshot {
            txn_info: Some(txn(2, 20, true)),
            ..SessionSnapshot::default()
        },
    );
    assert_eq!(manager.ShowTxnList(), [txn(2, 20, true)]);
}

#[test]
fn internal_sessions_track_identity_count_and_live_txn_timestamps() {
    let manager = MockSessionManager::default();
    manager.StoreInternalSession(1, Some(txn(1, 30, true)));
    manager.StoreInternalSession(2, None);
    assert!(manager.ContainsInternalSession(1));
    assert_eq!(manager.InternalSessionCount(), 2);
    assert_eq!(manager.GetInternalSessionStartTSList(), [30]);
    manager.DeleteInternalSession(1);
    assert!(!manager.ContainsInternalSession(1));
}

#[test]
fn server_attributes_status_and_old_ddl_jobs_match_go_contracts() {
    let manager = MockSessionManager::default();
    manager.SetServerID(42);
    let attrs = HashMap::from([(
        7,
        HashMap::from([("program".to_owned(), "mysql".to_owned())]),
    )]);
    manager.SetConAttrs(attrs.clone());
    manager.StoreConnection(
        7,
        SessionSnapshot {
            lock_ddl_jobs: HashSet::from([11]),
            ..SessionSnapshot::default()
        },
    );
    let mut jobs = HashSet::from([11, 12]);
    manager.CheckOldRunningTxn(&mut jobs);

    assert_eq!(manager.ServerID(), 42);
    assert_eq!(manager.GetConAttrs(), attrs);
    assert_eq!(jobs, HashSet::from([12]));
    assert!(manager.GetStatusVars().is_empty());
}
