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
fn go_merge_37_ia_json_keys_match_persisted_log_contract() {
    let mut record = StmtRecord::default();
    record.IAExecCount = 2;
    record.SumIARemoteReadSegmentCount = 3;
    record.MaxIARemoteReadSegmentCount = 2;
    record.SumIARemoteReadSegmentWaitTime = Duration::from_millis(5);
    record.MaxIARemoteReadSegmentWaitTime = Duration::from_millis(4);
    let json: serde_json::Value =
        serde_json::from_slice(&marshalStmtRecord(&record).unwrap()).unwrap();
    assert_eq!(json["ia_exec_count"], 2);
    assert_eq!(json["sum_ia_remote_read_segment_count"], 3);
    assert_eq!(json["max_ia_remote_read_segment_count"], 2);
    assert!(json.get("i_a_exec_count").is_none());
    assert!(json.get("sum_i_a_remote_read_segment_count").is_none());
    assert_eq!(json["sum_ia_remote_read_segment_wait_time"], 5_000_000);
    assert_eq!(json["max_ia_remote_read_segment_wait_time"], 4_000_000);
    let decoded: StmtRecord = serde_json::from_value(json).unwrap();
    assert_eq!(
        decoded.SumIARemoteReadSegmentWaitTime,
        Duration::from_millis(5)
    );
}

#[test]
fn go_merge_37_skips_empty_table_names_and_formats_digest_text() {
    let _guard = crate::testkit::SQL_LENGTH_TEST_LOCK.lock().unwrap();
    let mut info = GenerateStmtExecInfo4Test("digest");
    info.StmtCtx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "db0".into(),
            Table: "".into(),
        },
        TableEntry {
            DB: "DB1".into(),
            Table: "TABLE1".into(),
        },
        TableEntry {
            DB: "db2".into(),
            Table: "".into(),
        },
    ]);
    info.NormalizedSQL = "s".repeat(defaultMaxSQLLength as usize + 2);
    let record = NewStmtRecord(&info);
    assert_eq!(record.TableNames, "db1.table1");
    assert_eq!(
        record.NormalizedSQL,
        format!("{}(len:32770)", "s".repeat(32768))
    );
}

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
    let _guard = crate::testkit::SQL_LENGTH_TEST_LOCK.lock().unwrap();
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
    assert_eq!(items["ia_exec_count"], 0);
    assert!(items.get("ia_remote_exec_count").is_none());
    assert!(items.get("sum_ia_remote_read_segment_count").is_some());
    assert!(items.get("max_ia_remote_read_segment_count").is_some());

    let items: serde_json::Value =
        serde_json::from_slice(&marshalEvictedStmtRecord(&record2).unwrap()).unwrap();
    assert_eq!(items["additional_fields"]["stmt_meta_a"], "value_a");
    assert_eq!(items["evicted"], true);
    assert_eq!(items["digest"], record2.Digest);
    setStmtLogAdditionalFields(HashMap::new());
}
#[test]
fn go_merge_38_ru_version_selects_summary_values() {
    let raw = GenerateStmtExecInfo4Test("ru").RUDetail.unwrap();
    let v1 = SelectRUDetailsForStatementSummary(Some(raw.clone()), 1, Some(19.0), true).unwrap();
    assert_eq!((v1.RRU(), v1.WRU()), (1.2, 3.4));
    let v2_read =
        SelectRUDetailsForStatementSummary(Some(raw.clone()), 2, Some(19.0), false).unwrap();
    assert_eq!((v2_read.RRU(), v2_read.WRU()), (19.0, 0.0));
    let v2_write =
        SelectRUDetailsForStatementSummary(Some(raw.clone()), 2, Some(19.0), true).unwrap();
    assert_eq!((v2_write.RRU(), v2_write.WRU()), (0.0, 19.0));
    assert_eq!(v2_write.RUWaitDuration(), raw.RUWaitDuration());
    let pending = SelectRUDetailsForStatementSummary(Some(raw.clone()), 2, None, true).unwrap();
    assert_eq!((pending.RRU(), pending.WRU()), (raw.RRU(), raw.WRU()));
}

#[test]
fn ia_add_and_merge_preserve_independent_maxima_and_nil_scan() {
    let mut first_info = GenerateStmtExecInfo4Test("ia");
    let scan = first_info
        .ExecDetail
        .CopExecDetails
        .ScanDetail
        .as_mut()
        .unwrap();
    scan.IaRemoteReadSegmentCount = 3;
    scan.IaRemoteReadSegmentBytes = 8192;
    scan.IaRemoteReadSegmentDuration = Duration::from_millis(5);
    let mut second_info = GenerateStmtExecInfo4Test("ia");
    let scan = second_info
        .ExecDetail
        .CopExecDetails
        .ScanDetail
        .as_mut()
        .unwrap();
    scan.IaRemoteReadSegmentCount = 5;
    scan.IaRemoteReadSegmentBytes = 4096;
    scan.IaRemoteReadSegmentDuration = Duration::from_millis(9);
    let mut record = NewStmtRecord(&first_info);
    record.Add(&first_info);
    record.Add(&second_info);
    let mut nil_info = GenerateStmtExecInfo4Test("ia");
    nil_info.ExecDetail.CopExecDetails.ScanDetail = None;
    record.Add(&nil_info);
    let mut merged = NewStmtRecord(&first_info);
    merged.Merge(&record);
    for stats in [&record, &merged] {
        assert_eq!(stats.ExecCount, 3);
        assert_eq!(stats.SumIARemoteReadSegmentCount, 8);
        assert_eq!(stats.MaxIARemoteReadSegmentCount, 5);
        assert_eq!(stats.SumIARemoteReadSegmentSize, 12288);
        assert_eq!(stats.MaxIARemoteReadSegmentSize, 8192);
        assert_eq!(
            stats.SumIARemoteReadSegmentWaitTime,
            Duration::from_millis(14)
        );
        assert_eq!(
            stats.MaxIARemoteReadSegmentWaitTime,
            Duration::from_millis(9)
        );
    }
}
