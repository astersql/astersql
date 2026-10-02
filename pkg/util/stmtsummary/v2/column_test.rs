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

// 语句摘要列工厂（column factory）的集成测试。
//
// 对照 Go `TestColumn`：构造一组代表性列名，经 `makeColumnFactories` 生成取值闭包，
// 再对测试执行信息聚合后的 `StmtRecord` 逐列断言 Datum 内容。

#![allow(non_snake_case, non_upper_case_globals)]

use std::time::{Duration, UNIX_EPOCH};
use task_stmtsummary_v2::*;

#[test]
fn v2_unsigned_double_metrics() {
    let context = ColumnContext::new("instance", chrono_tz::UTC);
    let mut record = StmtRecord::default();
    record.ExecCount = 2;
    let large = 1_u64 << 63;
    record.SumRocksdbDeleteSkippedCount = large;
    record.SumRocksdbKeySkippedCount = large;
    record.SumRocksdbBlockCacheHitCount = large;
    record.SumRocksdbBlockReadCount = large;
    record.SumRocksdbBlockReadByte = large;
    record.SumAffectedRows = large;
    for name in [
        AvgRocksdbDeleteSkippedCountStr,
        AvgRocksdbKeySkippedCountStr,
        AvgRocksdbBlockCacheHitCountStr,
        AvgRocksdbBlockReadCountStr,
        AvgRocksdbBlockReadByteStr,
        AvgAffectedRowsStr,
    ] {
        let datum = makeColumnFactories(&[column(name)])[0](&context, &record).into_datum();
        assert_eq!(datum.GetFloat64(), large as f64 / 2.0, "{name}");
    }
    record.ExecCount = 1;
    record.SumRocksdbBlockCacheHitCount = 60;
    record.SumRocksdbBlockReadCount = 21103;
    for (name, expected) in [
        (AvgRocksdbBlockCacheHitCountStr, 60.0),
        (AvgRocksdbBlockReadCountStr, 21103.0),
    ] {
        let datum = makeColumnFactories(&[column(name)])[0](&context, &record).into_datum();
        assert_eq!(datum.GetFloat64(), expected, "{name}");
    }
    record.ExecCount = 0;
    let datum =
        makeColumnFactories(&[column(AvgAffectedRowsStr)])[0](&context, &record).into_datum();
    assert_eq!(datum.GetFloat64(), 0.0);
}

#[test]
fn go_merge_37_averages_all_unsigned_and_ia_columns() {
    let mut record = StmtRecord::default();
    record.ExecCount = 2;
    record.CommitCount = 0;
    let large = 1_u64 << 63;
    record.SumRocksdbDeleteSkippedCount = large;
    record.SumRocksdbKeySkippedCount = large;
    record.SumRocksdbBlockCacheHitCount = large;
    record.SumRocksdbBlockReadCount = large;
    record.SumRocksdbBlockReadByte = large;
    record.SumIARemoteReadSegmentCount = large;
    record.SumIARemoteReadSegmentSize = large;
    record.SumAffectedRows = large;
    record.IAExecCount = 1;
    record.MaxIARemoteReadSegmentCount = 3;
    record.MaxIARemoteReadSegmentSize = 4096;
    record.SumIARemoteReadSegmentWaitTime = Duration::from_millis(5);
    record.MaxIARemoteReadSegmentWaitTime = Duration::from_millis(5);
    record.SumKVTotal = Duration::from_nanos(10);
    record.SumPDTotal = Duration::from_nanos(20);
    record.SumBackoffTotal = Duration::from_nanos(30);
    record.SumWriteSQLRespTotal = Duration::from_nanos(40);
    let context = ColumnContext::new("", chrono_tz::UTC);
    for name in [
        AvgRocksdbDeleteSkippedCountStr,
        AvgRocksdbKeySkippedCountStr,
        AvgRocksdbBlockCacheHitCountStr,
        AvgRocksdbBlockReadCountStr,
        AvgRocksdbBlockReadByteStr,
        AvgIARemoteReadSegmentCountStr,
        AvgIARemoteReadSegmentSizeStr,
        AvgAffectedRowsStr,
    ] {
        let value = makeColumnFactories(&[column(name)])[0](&context, &record).into_datum();
        assert_eq!(value.GetFloat64(), large as f64 / 2.0, "{name}");
    }
    for (name, expected) in [
        (AvgKvTimeStr, 5),
        (AvgPdTimeStr, 10),
        (AvgBackoffTotalTimeStr, 15),
        (AvgWriteSQLRespTimeStr, 20),
        (AvgIARemoteReadSegmentWaitTimeStr, 2_500_000),
        (MaxIARemoteReadSegmentWaitTimeStr, 5_000_000),
    ] {
        let value = makeColumnFactories(&[column(name)])[0](&context, &record).into_datum();
        assert_eq!(value.GetInt64(), expected, "{name}");
    }
    for (name, expected) in [
        (IAExecCountStr, 1),
        (MaxIARemoteReadSegmentCountStr, 3),
        (MaxIARemoteReadSegmentSizeStr, 4096),
    ] {
        let value = makeColumnFactories(&[column(name)])[0](&context, &record).into_datum();
        assert_eq!(value.GetInt64(), expected, "{name}");
    }
}

/// 构造仅填充原始列名（`Name.O`）的 `ColumnInfo` 桩。
fn column(name: &str) -> model::ColumnInfo {
    let mut column = model::ColumnInfo::default();
    column.Name.O = name.to_owned();
    column
}

#[test]
fn go_merge_36_v2_columns_use_unsigned_float_and_execution_count() {
    let mut record = StmtRecord::default();
    record.ExecCount = 2;
    record.CommitCount = 1;
    record.SumRocksdbDeleteSkippedCount = i64::MAX as u64 + 3;
    record.SumAffectedRows = i64::MAX as u64 + 3;
    record.IAExecCount = 1;
    record.SumIARemoteReadSegmentCount = 3;
    record.SumKVTotal = Duration::from_nanos(10);
    let context = ColumnContext::new("instance", chrono_tz::UTC);
    let names = [
        AvgRocksdbDeleteSkippedCountStr,
        AvgAffectedRowsStr,
        IAExecCountStr,
        AvgIARemoteReadSegmentCountStr,
        AvgKvTimeStr,
    ];
    let columns = names.map(column);
    let values = makeColumnFactories(&columns)
        .into_iter()
        .map(|factory| factory(&context, &record).into_datum())
        .collect::<Vec<_>>();
    assert_eq!(
        values[0].GetFloat64(),
        record.SumRocksdbDeleteSkippedCount as f64 / 2.0
    );
    assert_eq!(values[1].GetFloat64(), record.SumAffectedRows as f64 / 2.0);
    assert_eq!(values[2].GetInt64(), 1);
    assert_eq!(values[3].GetFloat64(), 1.5);
    assert_eq!(values[4].GetInt64(), 5);
}

// TestColumn keeps the Go table and checks every requested factory result.
/// 校验实例地址、digest、延迟与 CPU 等列工厂输出与记录字段一致。
#[test]
fn TestColumn() {
    // 选取覆盖字符串、整数与纳秒延迟的代表性列集合。
    let columns = [
        column(ClusterTableInstanceColumnNameStr),
        column(StmtTypeStr),
        column(SchemaNameStr),
        column(DigestStr),
        column(DigestTextStr),
        column(TableNamesStr),
        column(IndexNamesStr),
        column(SampleUserStr),
        column(ExecCountStr),
        column(SumLatencyStr),
        column(MaxLatencyStr),
        column(AvgTidbCPUTimeStr),
        column(AvgTikvCPUTimeStr),
    ];
    let factories = makeColumnFactories(&columns);
    let info = GenerateStmtExecInfo4Test("digest");
    let mut record = NewStmtRecord(&info);
    record.Add(&info);
    let context = ColumnContext::new("instance_addr", chrono_tz::Asia::Shanghai);

    // 按列名分支断言工厂结果；未知列名直接 panic 暴露漏注册。
    for (column, factory) in columns.iter().zip(factories) {
        let value = factory(&context, &record).into_datum();
        match column.Name.O.as_str() {
            ClusterTableInstanceColumnNameStr => assert_eq!(value.GetString(), "instance_addr"),
            StmtTypeStr => assert_eq!(value.GetString(), record.StmtType),
            SchemaNameStr => assert_eq!(value.GetString(), record.SchemaName),
            DigestStr => assert_eq!(value.GetString(), record.Digest),
            DigestTextStr => assert_eq!(value.GetString(), record.NormalizedSQL),
            TableNamesStr => assert_eq!(value.GetString(), record.TableNames),
            IndexNamesStr => assert_eq!(value.GetString(), record.IndexNames.join(",")),
            SampleUserStr => assert_eq!(value.GetString(), info.User),
            ExecCountStr => assert_eq!(value.GetInt64(), 1),
            SumLatencyStr => assert_eq!(value.GetInt64(), record.SumLatency.as_nanos() as i64),
            MaxLatencyStr => assert_eq!(value.GetInt64(), record.MaxLatency.as_nanos() as i64),
            AvgTidbCPUTimeStr => {
                assert_eq!(value.GetInt64(), record.SumTidbCPU.as_nanos() as i64)
            }
            AvgTikvCPUTimeStr => {
                assert_eq!(value.GetInt64(), record.SumTikvCPU.as_nanos() as i64)
            }
            name => panic!("unexpected column {name}"),
        }
    }
}

// Go time.Time.Unix floors negative fractional timestamps instead of truncating
// them toward zero. Keep the statement-summary timestamp columns consistent.
#[test]
fn timestamp_before_unix_epoch_matches_go_flooring() {
    let columns = [column(FirstSeenStr)];
    let factory = makeColumnFactories(&columns)[0];
    let info = GenerateStmtExecInfo4Test("digest");
    let mut record = NewStmtRecord(&info);
    record.FirstSeen = UNIX_EPOCH - Duration::from_millis(500);

    let context = ColumnContext::new("instance_addr", chrono_tz::UTC);
    let value = factory(&context, &record).into_datum();

    let time = value.GetMysqlTime();
    assert_eq!(
        (
            time.Year(),
            time.Month(),
            time.Day(),
            time.Hour(),
            time.Minute(),
            time.Second(),
        ),
        (1969, 12, 31, 23, 59, 59)
    );
}
