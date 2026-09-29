// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 语句摘要记录 `StmtRecord` 的单元测试（对应 Go `record_test.go`）。
//
// 覆盖 `NewStmtRecord` 字段初始化、`Add`/`Merge` 聚合，以及序列化附加字段与驱逐标记。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::time::Duration;
use task_stmtsummary_v2::*;

#[test]
fn go_merge_36_v2_ia_stats_accumulate_and_merge() {
    let mut info = GenerateStmtExecInfo4Test("ia");
    let scan = info.ExecDetail.CopExecDetails.ScanDetail.as_mut().unwrap();
    scan.IaRemoteReadSegmentCount = 3;
    scan.IaRemoteReadSegmentBytes = 4096;
    scan.IaRemoteReadSegmentDuration = Duration::from_millis(5);
    let mut first = NewStmtRecord(&info);
    first.Add(&info);
    assert_eq!(first.IAExecCount, 1);
    assert_eq!(first.SumIARemoteReadSegmentCount, 3);
    assert_eq!(first.SumIARemoteReadSegmentSize, 4096);
    assert_eq!(
        first.SumIARemoteReadSegmentWaitTime,
        Duration::from_millis(5)
    );
    let mut merged = NewStmtRecord(&info);
    merged.Add(&info);
    merged.Merge(&first);
    assert_eq!(merged.IAExecCount, 2);
    assert_eq!(merged.SumIARemoteReadSegmentCount, 6);
    assert_eq!(merged.MaxIARemoteReadSegmentSize, 4096);
}

/// 校验新建记录字段、累加/合并指标，以及 marshal 附加字段与 evicted 标记。
#[test]
fn TestStmtRecord() {
    let info = GenerateStmtExecInfo4Test("digest1");
    let mut record1 = NewStmtRecord(&info);
    assert_eq!(info.SchemaName, record1.SchemaName);
    assert_eq!(info.Digest, record1.Digest);
    assert_eq!(info.PlanDigest, record1.PlanDigest);
    assert_eq!(info.StmtCtx.StmtType, record1.StmtType);
    assert_eq!(info.NormalizedSQL, record1.NormalizedSQL);
    assert_eq!(record1.TableNames, "db1.tb1,db2.tb2");
    assert_eq!(info.IsInternal, record1.IsInternal);
    assert_eq!(formatSQL(info.LazyInfo.GetOriginalSQL()), record1.SampleSQL);
    let (binding_sql, binding_digest) = info.LazyInfo.GetBindingSQLAndDigest();
    assert_eq!(binding_sql, record1.BindingSQL);
    assert_eq!(binding_digest, record1.BindingDigest);
    assert_eq!(info.Charset, record1.Charset);
    assert_eq!(info.Collation, record1.Collation);
    assert_eq!(info.PrevSQL, record1.PrevSQL);
    assert_eq!(*info.StmtCtx.IndexNames.lock().unwrap(), record1.IndexNames);
    assert_eq!(info.TotalLatency, record1.MinLatency);
    assert_eq!(info.Prepared, record1.Prepared);
    assert_eq!(info.StartTime, record1.FirstSeen);
    assert_eq!(info.StartTime, record1.LastSeen);
    assert_eq!(info.KeyspaceName, record1.KeyspaceName);
    assert_eq!(info.KeyspaceID, record1.KeyspaceID);
    assert!(record1.AuthUsers.is_empty());
    assert_eq!(record1.ExecCount, 0);
    assert_eq!(record1.SumLatency, Duration::ZERO);
    assert_eq!(record1.MaxLatency, Duration::ZERO);
    assert_eq!(info.ResourceGroupName, record1.ResourceGroupName);

    // 首次 Add：累计执行次数、延迟与 RU，并记录用户。
    record1.Add(&info);
    assert_eq!(record1.AuthUsers.len(), 1);
    assert!(record1.AuthUsers.contains("user"));
    assert_eq!(record1.ExecCount, 1);
    assert_eq!(record1.SumLatency, info.TotalLatency);
    assert_eq!(record1.MaxLatency, info.TotalLatency);
    assert_eq!(record1.MinLatency, info.TotalLatency);
    let (rru, wru, wait) = info
        .RUDetail
        .as_ref()
        .map_or((0.0, 0.0, Duration::ZERO), |detail| {
            (detail.RRU(), detail.WRU(), detail.RUWaitDuration())
        });
    assert_eq!(record1.MaxRRU, rru);
    assert_eq!(record1.SumRRU, rru);
    assert_eq!(record1.MaxWRU, wru);
    assert_eq!(record1.SumWRU, wru);
    assert_eq!(record1.MaxRUWaitDuration, wait);
    assert_eq!(record1.SumRUWaitDuration, wait);
    assert_eq!(record1.SumTidbCPU, info.CPUUsages.TidbCPUTime);
    assert_eq!(record1.SumTikvCPU, info.CPUUsages.TikvCPUTime);
    assert_eq!(record1.SumNumCopTasks, 10);
    assert_eq!(record1.CommitCount, 1);
    assert_eq!(record1.SumTotalKeys, 1_000);
    assert_eq!(record1.SumBackoffTimes, 1);
    assert_eq!(record1.BackoffTypes.get("txnlock"), Some(&1));
    assert_eq!(record1.SumRRU, 1.2);
    assert_eq!(record1.SumWRU, 3.4);
    assert_eq!(record1.SumRUWaitDuration, Duration::from_millis(2));
    assert_eq!(record1.SumTidbCPU, Duration::from_nanos(20));
    assert_eq!(record1.SumTikvCPU, Duration::from_nanos(10_000));

    // Merge 后执行次数与求和类指标应为两倍，max 仍为单次峰值。
    let mut record2 = NewStmtRecord(&info);
    record2.Add(&info);
    record2.Merge(&record1);
    assert_eq!(record2.AuthUsers.len(), 1);
    assert!(record2.AuthUsers.contains("user"));
    assert_eq!(record2.ExecCount, 2);
    assert_eq!(record2.SumLatency, info.TotalLatency * 2);
    assert_eq!(record2.MaxLatency, info.TotalLatency);
    assert_eq!(record2.MinLatency, info.TotalLatency);
    assert_eq!(record2.SumRRU, rru * 2.0);
    assert_eq!(record2.SumWRU, wru * 2.0);
    assert_eq!(record2.SumRUWaitDuration, wait * 2);
    assert_eq!(record2.SumTidbCPU, info.CPUUsages.TidbCPUTime * 2);
    assert_eq!(record2.SumTikvCPU, info.CPUUsages.TikvCPUTime * 2);

    // 日志序列化应嵌入 additional_fields；驱逐记录还需带 evicted=true。
    setStmtLogAdditionalFields(HashMap::from([("stmt_meta_a".into(), "value_a".into())]));
    let items: serde_json::Value =
        serde_json::from_slice(&marshalStmtRecord(&record2).unwrap()).unwrap();
    assert_eq!(items["additional_fields"]["stmt_meta_a"], "value_a");
    assert_eq!(items["digest"], record2.Digest);

    let items: serde_json::Value =
        serde_json::from_slice(&marshalEvictedStmtRecord(&record2).unwrap()).unwrap();
    assert_eq!(items["additional_fields"]["stmt_meta_a"], "value_a");
    assert_eq!(items["evicted"], true);
    assert_eq!(items["digest"], record2.Digest);
    setStmtLogAdditionalFields(HashMap::new());
}
