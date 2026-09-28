// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// sli 迁移回归：对照 Go 验证事务写入吞吐 SLI 累计、有效性边界与指标上报。
//
// 覆盖 String 格式、小/大事务分界、commit 后 reset，以及 failpoint 保留状态。

use std::time::Duration;

use astersql_util_sli::{TxnWriteThroughputSLI, failpoint, metrics, test_guard};

/// 累计写入/耗时后，String 格式与 IsSmallTxn 判断与 Go 一致。
#[test]
fn accumulates_state_and_matches_go_string_format() {
    let mut sli = TxnWriteThroughputSLI::default();
    sli.AddTxnWriteSize(58, 2);
    sli.FinishExecuteStmt(Duration::from_secs(1), 2, true);

    assert_eq!(
        sli.String(),
        "invalid: false, affectRow: 2, writeSize: 58, readKeys: 0, writeKeys: 2, writeTime: 1s"
    );
    assert!(!sli.IsInvalid());
    assert!(sli.IsSmallTxn());
}

/// 无效条件与小事务边界（影响行数、写入大小、显式 SetInvalid）匹配 Go。
#[test]
fn invalid_conditions_and_small_transaction_boundaries_match_go() {
    let mut sli = TxnWriteThroughputSLI::default();
    assert!(sli.IsInvalid());

    sli.AddTxnWriteSize(1024 * 1024, 1);
    sli.FinishExecuteStmt(Duration::from_nanos(1), 20, true);
    assert!(!sli.IsInvalid());
    assert!(sli.IsSmallTxn());

    sli.AddTxnWriteSize(1, 0);
    assert!(!sli.IsSmallTxn());

    sli.Reset();
    sli.AddTxnWriteSize(609, 21);
    sli.FinishExecuteStmt(Duration::from_secs(21), 21, true);
    assert!(!sli.IsSmallTxn());

    sli.Reset();
    sli.AddTxnWriteSize(1, 1);
    sli.AddReadKeys(2);
    sli.FinishExecuteStmt(Duration::from_secs(1), 1, true);
    assert!(sli.IsInvalid());

    sli.Reset();
    sli.AddTxnWriteSize(1, 1);
    sli.FinishExecuteStmt(Duration::from_secs(1), 1, true);
    sli.SetInvalid();
    assert!(sli.IsInvalid());
}

/// commit（inTxn=false）上报小事务耗时后 Reset；failpoint 关闭时状态清零。
#[test]
fn commit_reports_small_duration_then_resets() {
    let _guard = test_guard();
    metrics::reset();
    failpoint::disable();
    let mut sli = TxnWriteThroughputSLI::default();
    sli.AddTxnWriteSize(58, 2);
    sli.FinishExecuteStmt(Duration::from_millis(750), 2, true);
    sli.FinishExecuteStmt(Duration::from_millis(250), 0, false);

    assert_eq!(metrics::small_txn_observations(), (1, 1.0));
    assert_eq!(metrics::throughput_observations(), (0, 0.0));
    assert_eq!(
        sli.String(),
        "invalid: false, affectRow: 0, writeSize: 0, readKeys: 0, writeKeys: 0, writeTime: 0s"
    );
}

/// Go 的 21 行大事务时序：0 影响行语句不计时，commit 计时并上报吞吐。
#[test]
fn large_transaction_reports_throughput_and_failpoint_preserves_state() {
    let _guard = test_guard();
    metrics::reset();
    failpoint::enable();
    let mut sli = TxnWriteThroughputSLI::default();
    sli.AddTxnWriteSize(609, 21);
    sli.FinishExecuteStmt(Duration::from_secs(21), 21, true);
    sli.FinishExecuteStmt(Duration::from_secs(1), 0, true);
    sli.FinishExecuteStmt(Duration::from_secs(1), 0, false);

    assert_eq!(metrics::small_txn_observations(), (0, 0.0));
    assert_eq!(metrics::throughput_observations(), (1, 609.0 / 22.0));
    assert_eq!(
        sli.String(),
        "invalid: false, affectRow: 21, writeSize: 609, readKeys: 0, writeKeys: 21, writeTime: 22s"
    );
    failpoint::disable();
}

/// 无效事务（读键多于写键等）不向 metrics 上报任何观测。
#[test]
fn invalid_transaction_does_not_report_metrics() {
    let _guard = test_guard();
    metrics::reset();
    failpoint::enable();
    let mut sli = TxnWriteThroughputSLI::default();
    sli.AddTxnWriteSize(16, 1);
    sli.AddReadKeys(2);
    sli.FinishExecuteStmt(Duration::from_secs(1), 1, false);

    assert_eq!(metrics::small_txn_observations(), (0, 0.0));
    assert_eq!(metrics::throughput_observations(), (0, 0.0));
    failpoint::disable();
}
