// Copyright 2026 AsterSQL.

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

//! Go parity tests for telemetry snapshots backed by metrics owned by other modules.

use astersql_metrics::telemetry::{
    GetFairLockingUsageCounter, GetLazyPessimisticUniqueCheckSetCounter,
    GetNonTransactionalStmtCounter, GetSavepointStmtCounter,
};

#[test]
fn telemetry_getters_read_the_shared_session_and_executor_metrics() {
    crate::main_test::ensure_test_env();
    unsafe { astersql_metrics::metrics::InitMetrics().expect("initialize metrics package") };

    let dml_before = GetNonTransactionalStmtCounter();
    let savepoint_before = GetSavepointStmtCounter();
    let lazy_before = GetLazyPessimisticUniqueCheckSetCounter();
    let fair_before = GetFairLockingUsageCounter();

    unsafe {
        astersql_metrics::session::NonTransactionalDMLCount
            .as_ref()
            .unwrap()
            .with_label_values(&["delete"])
            .inc();
        astersql_metrics::executor::StmtNodeCounter
            .as_ref()
            .unwrap()
            .with_label_values(&["Savepoint", "", "default"])
            .inc();
        astersql_metrics::session::LazyPessimisticUniqueCheckSetCount
            .as_ref()
            .unwrap()
            .inc();
        astersql_metrics::session::FairLockingUsageCount
            .as_ref()
            .unwrap()
            .with_label_values(&[astersql_metrics::session::LblFairLockingTxnUsed])
            .inc();
    }

    assert_eq!(
        GetNonTransactionalStmtCounter().DeleteCount,
        dml_before.DeleteCount + 1
    );
    assert_eq!(GetSavepointStmtCounter(), savepoint_before + 1);
    assert_eq!(GetLazyPessimisticUniqueCheckSetCounter(), lazy_before + 1);
    assert_eq!(
        GetFairLockingUsageCounter().TxnFairLockingUsed,
        fair_before.TxnFairLockingUsed + 1
    );
}
