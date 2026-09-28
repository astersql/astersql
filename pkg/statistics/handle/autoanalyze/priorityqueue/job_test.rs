// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 分析作业（AnalysisJob）公共行为单元测试。
//
// 覆盖各类作业 String() 输出与 Go 对齐、动态分区类型判定，
// 以及 AsJSONIndicators 的百分号与 Go 风格时长格式。

use std::collections::HashMap;
use std::time::Duration;

use crate::{
    AnalysisJob, AsJSONIndicators, DynamicPartitionedTableAnalysisJob, Indicators,
    IsDynamicPartitionedTableAnalysisJob, NewDynamicPartitionedTableAnalysisJob,
    NewNonPartitionedTableAnalysisJob, NewStaticPartitionTableAnalysisJob,
};

/// 构造仅设置 ChangePercentage 的指标，其余为零。
fn indicators(change_percentage: f64) -> Indicators {
    Indicators {
        ChangePercentage: change_percentage,
        TableSize: 0.0,
        LastAnalysisDuration: Duration::ZERO.into(),
    }
}

// TestStringer 对应 Go 的表驱动 String() 测试，逐 case 比对完整输出。
/// 表驱动：非分区 / 动态分区 / 静态分区及其索引作业的 String() 全文比对。
#[test]
fn TestStringer() {
    /// 单个 String() 用例：作业实例与期望输出。
    struct TestCase {
        name: &'static str,
        job: Box<dyn AnalysisJob>,
        want: &'static str,
    }

    let mut non_partitioned =
        NewNonPartitionedTableAnalysisJob(1, HashMap::new(), 2, false, 0.0, 0.0, Duration::ZERO);
    non_partitioned.SchemaName = "test_schema".to_owned();
    non_partitioned.TableName = "test_table".to_owned();
    non_partitioned.Weight = 1.999999;
    non_partitioned.indicators = indicators(0.5);

    let mut non_partitioned_index = NewNonPartitionedTableAnalysisJob(
        2,
        HashMap::from([(1i64, ())]),
        2,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    non_partitioned_index.SchemaName = "test_schema".to_owned();
    non_partitioned_index.TableName = "test_table".to_owned();
    non_partitioned_index.IndexNames = vec!["idx".to_owned()];
    non_partitioned_index.Weight = 1.999999;
    non_partitioned_index.indicators = indicators(0.5);

    let mut dynamic_partition = NewDynamicPartitionedTableAnalysisJob(
        3,
        HashMap::from([(1i64, ()), (2i64, ())]),
        HashMap::new(),
        2,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    dynamic_partition.SchemaName = "test_schema".to_owned();
    dynamic_partition.GlobalTableName = "test_table".to_owned();
    dynamic_partition.PartitionNames = vec!["p0".to_owned(), "p1".to_owned()];
    dynamic_partition.Weight = 1.999999;
    dynamic_partition.indicators = indicators(0.5);

    let mut dynamic_partition_index = NewDynamicPartitionedTableAnalysisJob(
        4,
        HashMap::new(),
        HashMap::from([(1i64, vec![1i64, 2i64]), (2i64, vec![1i64, 2i64])]),
        2,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    dynamic_partition_index.SchemaName = "test_schema".to_owned();
    dynamic_partition_index.GlobalTableName = "test_table".to_owned();
    dynamic_partition_index.PartitionIndexNames =
        HashMap::from([("idx".to_owned(), vec!["p0".to_owned(), "p1".to_owned()])]);
    dynamic_partition_index.Weight = 1.999999;
    dynamic_partition_index.indicators = indicators(0.5);

    let mut static_partition = NewStaticPartitionTableAnalysisJob(
        5,
        6,
        HashMap::new(),
        2,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    static_partition.SchemaName = "test_schema".to_owned();
    static_partition.GlobalTableName = "test_table".to_owned();
    static_partition.StaticPartitionName = "p0".to_owned();
    static_partition.Weight = 1.999999;
    static_partition.indicators = indicators(0.5);

    let mut static_partition_index = NewStaticPartitionTableAnalysisJob(
        7,
        8,
        HashMap::from([(1i64, ())]),
        2,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    static_partition_index.SchemaName = "test_schema".to_owned();
    static_partition_index.GlobalTableName = "test_table".to_owned();
    static_partition_index.StaticPartitionName = "p0".to_owned();
    static_partition_index.IndexNames = vec!["idx".to_owned()];
    static_partition_index.Weight = 1.999999;
    static_partition_index.indicators = indicators(0.5);

    // 六种作业形态的期望字符串与 Go String() 完全一致。
    let tests = vec![
        TestCase {
            name: "analyze non-partitioned table",
            job: Box::new(non_partitioned),
            want: "NonPartitionedTableAnalysisJob:\n\tAnalyzeType: analyzeTable\n\tIndexes: \n\tSchema: test_schema\n\tTable: test_table\n\tTableID: 1\n\tTableStatsVer: 2\n\tChangePercentage: 0.500000\n\tTableSize: 0.00\n\tLastAnalysisDuration: 0s\n\tWeight: 1.999999\n",
        },
        TestCase {
            name: "analyze non-partitioned table index",
            job: Box::new(non_partitioned_index),
            want: "NonPartitionedTableAnalysisJob:\n\tAnalyzeType: analyzeIndex\n\tIndexes: idx\n\tSchema: test_schema\n\tTable: test_table\n\tTableID: 2\n\tTableStatsVer: 2\n\tChangePercentage: 0.500000\n\tTableSize: 0.00\n\tLastAnalysisDuration: 0s\n\tWeight: 1.999999\n",
        },
        TestCase {
            name: "analyze dynamic partition",
            job: Box::new(dynamic_partition),
            want: "DynamicPartitionedTableAnalysisJob:\n\tAnalyzeType: analyzeDynamicPartition\n\tPartitions: p0, p1\n\tPartitionIndexes: map[]\n\tSchema: test_schema\n\tGlobal Table: test_table\n\tGlobal TableID: 3\n\tTableStatsVer: 2\n\tChangePercentage: 0.500000\n\tTableSize: 0.00\n\tLastAnalysisDuration: 0s\n\tWeight: 1.999999\n",
        },
        TestCase {
            name: "analyze dynamic partition's indexes",
            job: Box::new(dynamic_partition_index),
            want: "DynamicPartitionedTableAnalysisJob:\n\tAnalyzeType: analyzeDynamicPartitionIndex\n\tPartitions: \n\tPartitionIndexes: map[idx:[p0 p1]]\n\tSchema: test_schema\n\tGlobal Table: test_table\n\tGlobal TableID: 4\n\tTableStatsVer: 2\n\tChangePercentage: 0.500000\n\tTableSize: 0.00\n\tLastAnalysisDuration: 0s\n\tWeight: 1.999999\n",
        },
        TestCase {
            name: "analyze static partition",
            job: Box::new(static_partition),
            want: "StaticPartitionedTableAnalysisJob:\n\tAnalyzeType: analyzeStaticPartition\n\tIndexes: \n\tSchema: test_schema\n\tGlobalTable: test_table\n\tGlobalTableID: 5\n\tStaticPartition: p0\n\tStaticPartitionID: 6\n\tTableStatsVer: 2\n\tChangePercentage: 0.500000\n\tTableSize: 0.00\n\tLastAnalysisDuration: 0s\n\tWeight: 1.999999\n",
        },
        TestCase {
            name: "analyze static partition's index",
            job: Box::new(static_partition_index),
            want: "StaticPartitionedTableAnalysisJob:\n\tAnalyzeType: analyzeStaticPartitionIndex\n\tIndexes: idx\n\tSchema: test_schema\n\tGlobalTable: test_table\n\tGlobalTableID: 7\n\tStaticPartition: p0\n\tStaticPartitionID: 8\n\tTableStatsVer: 2\n\tChangePercentage: 0.500000\n\tTableSize: 0.00\n\tLastAnalysisDuration: 0s\n\tWeight: 1.999999\n",
        },
    ];

    for tt in tests {
        assert_eq!(tt.want, tt.job.String(), "{}", tt.name);
    }
}

// TestIsDynamicPartitionedTableAnalysisJob 对应 Go 的表驱动类型判定测试。
/// 非动态分区作业返回 false，动态分区作业返回 true。
#[test]
fn TestIsDynamicPartitionedTableAnalysisJob() {
    let non_partitioned =
        NewNonPartitionedTableAnalysisJob(0, HashMap::new(), 0, false, 0.0, 0.0, Duration::ZERO);
    assert!(
        !IsDynamicPartitionedTableAnalysisJob(&non_partitioned),
        "non-partitioned table"
    );

    let dynamic: DynamicPartitionedTableAnalysisJob = NewDynamicPartitionedTableAnalysisJob(
        0,
        HashMap::new(),
        HashMap::new(),
        0,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    assert!(
        IsDynamicPartitionedTableAnalysisJob(&dynamic),
        "dynamic partitioned table"
    );
}

/// AsJSONIndicators：变更比例转百分号、时长按 Go `1m30s` 风格输出。
#[test]
fn indicators_json_keeps_go_field_semantics() {
    let json = AsJSONIndicators(&Indicators {
        ChangePercentage: 0.625,
        TableSize: 1234.0,
        LastAnalysisDuration: Duration::from_secs(90).into(),
    });
    assert_eq!("62.50%", json.ChangePercentage);
    assert_eq!("1234.00", json.TableSize);
    assert_eq!("1m30s", json.LastAnalysisDuration);
}

// Go job.go: query ordering, exact errors, and strict cooldown boundaries.
#[test]
fn cooldown_matches_go_branches_and_query_order() {
    use crate::job::{AnalysisRuntime, IsValidToAnalyze, TableMetadata};
    use std::sync::Mutex;

    struct Runtime {
        failed: Result<Option<Duration>, String>,
        average: Result<Option<Duration>, String>,
        calls: Mutex<Vec<&'static str>>,
    }
    impl AnalysisRuntime for Runtime {
        fn table_by_id(&self, _: i64) -> Option<TableMetadata> {
            panic!("unexpected metadata lookup")
        }
        fn last_failed_analysis_duration(
            &self,
            schema: &str,
            table: &str,
            partitions: &[String],
        ) -> Result<Option<Duration>, String> {
            assert_eq!((schema, table), ("db", "table"));
            assert_eq!(partitions, &["p1", "p0"]);
            self.calls.lock().unwrap().push("failed");
            self.failed.clone()
        }
        fn average_analysis_duration(
            &self,
            schema: &str,
            table: &str,
            partitions: &[String],
        ) -> Result<Option<Duration>, String> {
            assert_eq!((schema, table), ("db", "table"));
            assert_eq!(partitions, &["p1", "p0"]);
            self.calls.lock().unwrap().push("average");
            self.average.clone()
        }
        fn execute_analyze(&self, _: &str, _: &[String], _: i32, _: bool) -> Result<bool, String> {
            panic!("unexpected execution")
        }
    }
    let seconds = |n| Ok(Some(Duration::from_secs(n)));
    let twice = "last failed analysis duration is less than 2 times the average analysis duration";
    let cases = [
        (
            Err("history unavailable".into()),
            Err("must not query".into()),
            "fail to get last failed analysis duration: history unavailable",
        ),
        (
            seconds(0),
            Err("average unavailable".into()),
            "fail to get average analysis duration: average unavailable",
        ),
        (Ok(None), Ok(None), ""),
        (Ok(None), seconds(100), ""),
        (seconds(0), Ok(None), "last analysis just failed"),
        (
            seconds(1799),
            Ok(None),
            "last failed analysis duration is less than 30m0s",
        ),
        (seconds(1800), Ok(None), ""),
        (seconds(1801), Ok(None), ""),
        (
            seconds(9_223_372_037),
            Ok(None),
            "last failed analysis duration is less than 30m0s",
        ),
        (seconds(19), seconds(10), twice),
        (seconds(20), seconds(10), ""),
        (seconds(21), seconds(10), ""),
        (seconds(1), seconds(0), ""),
        // Go time.Duration multiplication wraps its signed int64 nanoseconds.
        (
            seconds(1),
            Ok(Some(Duration::from_nanos(i64::MAX as u64))),
            "",
        ),
    ];
    for (failed, average, reason) in cases {
        let short_circuit = failed.is_err();
        let runtime = Runtime {
            failed,
            average,
            calls: Mutex::new(vec![]),
        };
        assert_eq!(
            IsValidToAnalyze(&runtime, "db", "table", &["p1".into(), "p0".into()]),
            (reason.is_empty(), reason.into())
        );
        assert_eq!(
            *runtime.calls.lock().unwrap(),
            if short_circuit {
                vec!["failed"]
            } else {
                vec!["failed", "average"]
            }
        );
    }
    use astersql_statistics_handle_logutil::log::{LogField, LogLevel};
    use astersql_statistics_handle_logutil::{StatsErrVerboseSampleLogger, StatsSampleLogger};
    for (logger, level, message) in [
        (
            StatsErrVerboseSampleLogger(),
            LogLevel::Warn,
            "Fail to get last failed analysis duration",
        ),
        (
            StatsErrVerboseSampleLogger(),
            LogLevel::Warn,
            "Fail to get average analysis duration",
        ),
        (
            StatsSampleLogger(),
            LogLevel::Info,
            "Skip analysis because the last analysis just failed",
        ),
        (
            StatsSampleLogger(),
            LogLevel::Info,
            "Skip analysis because the last failed analysis duration is less than 30m0s",
        ),
        (
            StatsSampleLogger(),
            LogLevel::Info,
            "Skip analysis because the last failed analysis duration is less than 2 times the average analysis duration",
        ),
    ] {
        let entries = logger.entries();
        let entry = entries
            .iter()
            .find(|entry| entry.message == message)
            .expect("Go cooldown branch must emit its sampled statistics log");
        assert_eq!(entry.level, level);
        for key in ["category", "sampled", "schema", "table", "partitions"] {
            assert!(
                entry.fields.iter().any(|field| field.key() == key),
                "missing {key}"
            );
        }
        assert!(
            entry
                .fields
                .contains(&LogField::String("category".into(), "stats".into()))
        );
        if level == LogLevel::Warn {
            assert!(entry.fields.iter().any(|field| field.key() == "error"));
        } else if message.contains("less than") {
            for key in ["lastFailedAnalysisDuration", "averageAnalysisDuration"] {
                assert!(
                    entry.fields.iter().any(|field| field.key() == key),
                    "missing {key}"
                );
            }
        }
    }
}

#[test]
fn indicators_json_matches_go_nonfinite_and_signed_zero() {
    for (value, want) in [
        (f64::INFINITY, "+Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (f64::NAN, "NaN"),
        (-0.0, "-0.00"),
    ] {
        let json = AsJSONIndicators(&Indicators {
            ChangePercentage: value,
            TableSize: value,
            LastAnalysisDuration: Duration::ZERO.into(),
        });
        assert_eq!(json.TableSize, want);
        assert_eq!(json.ChangePercentage, format!("{want}%"));
    }
}

#[test]
fn duration_and_map_format_match_go() {
    use crate::job::{format_go_duration, format_go_string_list_map};
    for (nanos, want) in [
        (0, "0s"),
        (1, "1ns"),
        (999, "999ns"),
        (1000, "1µs"),
        (1001, "1.001µs"),
        (1_000_001, "1.000001ms"),
        (1_000_000_001, "1.000000001s"),
        (60_000_000_000, "1m0s"),
        (3_600_000_000_001, "1h0m0.000000001s"),
        (i64::MAX as u64, "2562047h47m16.854775807s"),
    ] {
        assert_eq!(format_go_duration(Duration::from_nanos(nanos)), want);
    }
    assert_eq!(format_go_string_list_map(&HashMap::new()), "map[]");
    assert_eq!(
        format_go_string_list_map(&HashMap::from([
            ("z".into(), vec![]),
            ("a".into(), vec!["p1".into(), "p0".into()])
        ])),
        "map[a:[p1 p0] z:[]]"
    );
}

#[test]
fn index_names_preserves_metadata_order_and_skips_missing_ids() {
    use crate::job::{IndexMetadata, TableMetadata, index_names};
    let metadata = TableMetadata {
        indices: vec![
            IndexMetadata {
                id: 2,
                name: "second".into(),
                ..Default::default()
            },
            IndexMetadata {
                id: 1,
                name: "first".into(),
                ..Default::default()
            },
            IndexMetadata {
                id: 3,
                name: "unrequested".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        index_names(&metadata, &HashMap::from([(1, ()), (2, ()), (99, ())])),
        vec!["second", "first"]
    );
    assert!(index_names(&metadata, &HashMap::new()).is_empty());
}

#[test]
fn hooks_receive_complete_jobs_for_all_job_types() {
    use crate::{AnalysisRuntime, TableMetadata};
    use std::sync::{Arc, Mutex};
    struct Runtime(bool);
    impl AnalysisRuntime for Runtime {
        fn table_by_id(&self, _: i64) -> Option<TableMetadata> {
            None
        }
        fn last_failed_analysis_duration(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn average_analysis_duration(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn execute_analyze(&self, _: &str, _: &[String], _: i32, _: bool) -> Result<bool, String> {
            Ok(self.0)
        }
    }
    let mut dynamic = NewDynamicPartitionedTableAnalysisJob(
        4,
        HashMap::from([(40, ())]),
        HashMap::new(),
        2,
        false,
        0.5,
        10.0,
        Duration::ZERO,
    );
    dynamic.PartitionNames = vec!["p0".into()];
    let mut jobs: Vec<Box<dyn AnalysisJob>> = vec![
        Box::new(NewNonPartitionedTableAnalysisJob(
            1,
            HashMap::new(),
            2,
            false,
            0.5,
            10.0,
            Duration::ZERO,
        )),
        Box::new(NewStaticPartitionTableAnalysisJob(
            2,
            3,
            HashMap::new(),
            2,
            false,
            0.5,
            10.0,
            Duration::ZERO,
        )),
        Box::new(dynamic),
    ];
    for job in &mut jobs {
        job.SetWeight(42.5);
        let expected = job.AsJSON();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let success = observed.clone();
        job.RegisterSuccessHook(Arc::new(move |job| {
            success.lock().unwrap().push((job.AsJSON(), None));
            job.SetWeight(99.0);
        }));
        let failure = observed.clone();
        job.RegisterFailureHook(Arc::new(move |job, retry| {
            failure.lock().unwrap().push((job.AsJSON(), Some(retry)));
        }));
        job.Analyze(&Runtime(true)).unwrap();
        assert_eq!(job.GetWeight(), 99.0);
        job.SetWeight(42.5);
        job.Analyze(&Runtime(false)).unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            vec![(expected.clone(), None), (expected, Some(true))]
        );
    }
}

#[test]
fn signed_duration_format_and_weight_match_go() {
    use crate::{AnalysisDuration, NewPriorityCalculator};
    for (nanos, text) in [
        (i64::MIN, "-2562047h47m16.854775808s"),
        (-1, "-1ns"),
        (-1_001, "-1.001µs"),
        (-30_000_000_000, "-30s"),
    ] {
        let duration = AnalysisDuration::from_nanos(nanos);
        assert_eq!(crate::job::format_go_duration(duration), text);
        let json = AsJSONIndicators(&Indicators {
            LastAnalysisDuration: duration,
            ..Default::default()
        });
        assert_eq!(json.LastAnalysisDuration, text);
    }
    let job = NewNonPartitionedTableAnalysisJob(
        1,
        HashMap::new(),
        2,
        false,
        0.5,
        10.0,
        AnalysisDuration::from_secs(-30),
    );
    assert!(
        NewPriorityCalculator().CalculateWeight(&job).is_nan(),
        "Go math.Sqrt of a negative interval is NaN"
    );
    assert!(job.String().contains("LastAnalysisDuration: -30s"));
}
