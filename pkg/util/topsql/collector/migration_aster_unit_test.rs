// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL collector（CPU 采样收集器）迁移基线单元测试。
//
// 覆盖：`sqlStats::tune` 分支、按 SQL/Plan digest 聚合 pprof Profile、
// 进程级 CPU 画像解析、Profile 分发到 Collector/Updater，以及 Start/Stop
// 与 ProfileContext 标签注入行为；与 Go 侧语义对齐。
// 审计映射：tune 对应 main_test.go::TestSQLStatsTune 与 cpu.go::sqlStats.tune；
// 聚合/空画像/分发分别对应 cpu.go::parseCPUProfileBySQLLabels、
// createSQLStats、parseCPUProfileForProcess、handleProfileData；生命周期对应
// Start/Stop/collectSQLCPULoop，标签对应三个 CtxWith* 函数。
// RecordingCollector/Updater 和 profile_with_samples 为 Rust 自研测试支撑，
// 对应 Go mockCollector/mockUpdater 的观察边界，不替换待测聚合或后台循环。

use super::*;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pprof::protos::{Label, Message, Profile, Sample, ValueType};
use serial_test::serial;

/// 记录每次 `Collect` 收到的 SQL CPU 时间批次，便于断言分发结果。
#[derive(Default)]
struct RecordingCollector {
    batches: Mutex<Vec<Vec<SQLCPUTimeRecord>>>,
}

impl Collector for RecordingCollector {
    fn Collect(&self, stats: Vec<SQLCPUTimeRecord>) {
        self.batches.lock().unwrap().push(stats);
    }
}

/// 记录进程级 CPU 时间更新调用（连接 ID、SQL ID、耗时）。
#[derive(Default)]
struct RecordingUpdater {
    records: Mutex<Vec<(u64, u64, Duration)>>,
}

impl ProcessCPUTimeUpdater for RecordingUpdater {
    fn UpdateProcessCPUTime(&self, connID: u64, sqlID: u64, cpuTime: Duration) {
        self.records.lock().unwrap().push((connID, sqlID, cpuTime));
    }
}

/// 构造带标签的假 pprof Profile：将键值写入 string_table 并挂到 Sample。
fn profile_with_samples(samples: Vec<(Vec<(&str, &str)>, i64)>) -> Profile {
    let mut profile = Profile {
        sample_type: vec![ValueType { ty: 0, unit: 0 }],
        string_table: vec![String::new()],
        ..Profile::default()
    };
    // 将 (key, value) 编码为 Profile 的 string_table 索引标签。
    for (labels, value) in samples {
        let mut encoded_labels = Vec::new();
        for (key, label_value) in labels {
            let key_index = profile.string_table.len() as i64;
            profile.string_table.push(key.to_owned());
            let str_index = profile.string_table.len() as i64;
            profile.string_table.push(label_value.to_owned());
            encoded_labels.push(Label {
                key: key_index,
                str: str_index,
                num: 0,
                num_unit: 0,
            });
        }
        profile.sample.push(Sample {
            location_id: Vec::new(),
            value: vec![value],
            label: encoded_labels,
        });
    }
    profile
}

/// 校验 `tune` 在无 plan / 单 plan / 多 plan 余量 / 无余量时与 Go 分支一致。
#[test]
fn sql_stats_tune_matches_all_go_branches() {
    let mut no_plan = sqlStats {
        plans: HashMap::new(),
        total: 100,
    };
    no_plan.tune();
    assert_eq!(no_plan.plans, HashMap::from([(String::new(), 100)]));

    let mut one_plan = sqlStats {
        plans: HashMap::from([("plan-1".to_owned(), 80)]),
        total: 100,
    };
    one_plan.tune();
    assert_eq!(one_plan.total, 100);
    assert_eq!(one_plan.plans, HashMap::from([("plan-1".to_owned(), 100)]));

    let mut two_plans = sqlStats {
        plans: HashMap::from([("plan-1".to_owned(), 30), ("plan-2".to_owned(), 30)]),
        total: 100,
    };
    two_plans.tune();
    assert_eq!(two_plans.total, 100);
    assert_eq!(
        two_plans.plans,
        HashMap::from([
            ("plan-1".to_owned(), 30),
            ("plan-2".to_owned(), 30),
            (String::new(), 40),
        ])
    );

    let mut no_remainder = sqlStats {
        plans: HashMap::from([("plan-1".to_owned(), 60), ("plan-2".to_owned(), 60)]),
        total: 100,
    };
    no_remainder.tune();
    assert_eq!(no_remainder.total, 100);
    assert_eq!(
        no_remainder.plans,
        HashMap::from([("plan-1".to_owned(), 60), ("plan-2".to_owned(), 60),])
    );
    no_remainder.total = 120;
    no_remainder.tune();
    assert_eq!(no_remainder.plans.len(), 2);
    assert_eq!(no_remainder.plans["plan-1"], 60);
    assert_eq!(no_remainder.plans["plan-2"], 60);
}

/// 校验按 SQL/Plan digest 聚合：十六进制解码、无 plan 归并、非法/无标签样本忽略。
#[test]
fn sql_profile_aggregation_decodes_digests_and_ignores_unlabelled_samples() {
    let profile = profile_with_samples(vec![
        (
            vec![(labelSQLDigest, "73716c"), (labelPlanDigest, "706c616e31")],
            30_000_000,
        ),
        (
            vec![(labelSQLDigest, "73716c"), (labelPlanDigest, "706c616e32")],
            30_000_000,
        ),
        (vec![(labelSQLDigest, "73716c")], 40_000_000),
        (
            vec![
                (labelSQLDigest, "7365636f6e64"),
                (labelPlanDigest, "706c616e"),
            ],
            20_000_000,
        ),
        (vec![(labelSQLDigest, "7365636f6e64")], 30_000_000),
        (vec![(labelSQLDigest, "not-hex")], 90_000_000),
        (vec![("ignored", "anything")], 500_000_000),
    ]);
    let mut stats = parseCPUProfileBySQLLabels(&profile);
    stats.sort_by(|left, right| {
        left.SQLDigest
            .cmp(&right.SQLDigest)
            .then(left.PlanDigest.cmp(&right.PlanDigest))
    });

    assert_eq!(stats.len(), 4);
    assert_eq!(stats[0].SQLDigest, b"second");
    assert_eq!(stats[0].PlanDigest, b"plan");
    assert_eq!(stats[0].CPUTimeMs, 50);
    assert_eq!(stats[1].SQLDigest, b"sql");
    assert!(stats[1].PlanDigest.is_empty());
    assert_eq!(stats[1].CPUTimeMs, 40);
    assert_eq!(stats[2].PlanDigest, b"plan1");
    assert_eq!(stats[2].CPUTimeMs, 30);
    assert_eq!(stats[3].PlanDigest, b"plan2");
    assert_eq!(stats[3].CPUTimeMs, 30);
}

/// 校验进程画像按连接保留最新 SQL，并累加同 SQLUID 的采样值。
#[test]
fn process_profile_keeps_latest_sql_per_connection() {
    let profile = profile_with_samples(vec![
        (vec![(labelSQLUID, "7_1")], 11),
        (vec![(labelSQLUID, "7_2")], 20),
        (vec![(labelSQLUID, "7_2")], 30),
        (vec![(labelSQLUID, "8_4")], 40),
        // Reverse traversal must replace the lower ID encountered first.
        (vec![(labelSQLUID, "8_3")], 90),
        (vec![("ignored", "anything")], 99),
    ]);
    let mut records = parseCPUProfileForProcess(&profile);
    records.sort_by_key(|record| record.0);
    assert_eq!(records, vec![(7, 2, 50), (8, 4, 40)]);
}

/// 校验 `handleProfileData` 同时向 Collector 与 ProcessCPUTimeUpdater 分发结果。
#[test]
fn profile_data_dispatches_sql_and_process_results() {
    let collector = Arc::new(RecordingCollector::default());
    let updater = Arc::new(RecordingUpdater::default());
    let profile = profile_with_samples(vec![(
        vec![(labelSQLDigest, "73716c"), (labelSQLUID, "9_3")],
        2_500_000,
    )]);
    let data = cpuprofile::ProfileData::success(profile.encode_to_vec());

    handleProfileData(&data, &collector, Some(&updater));

    let batches = collector.batches.lock().unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0],
        vec![SQLCPUTimeRecord {
            SQLDigest: b"sql".to_vec(),
            PlanDigest: Vec::new(),
            CPUTimeMs: 2,
        }]
    );
    assert_eq!(
        updater.records.lock().unwrap().as_slice(),
        &[(9, 3, Duration::from_nanos(2_500_000))]
    );
}

/// Go handleProfileData returns before either callback when parsing fails.
#[test]
fn invalid_profile_does_not_dispatch_partial_results() {
    let collector = Arc::new(RecordingCollector::default());
    let updater = Arc::new(RecordingUpdater::default());
    let data = cpuprofile::ProfileData::success(vec![0xff]);
    handleProfileData(&data, &collector, Some(&updater));
    assert!(collector.batches.lock().unwrap().is_empty());
    assert!(updater.records.lock().unwrap().is_empty());
}

/// 空 Profile 在 Go 中不会进入 sample 循环，应返回空结果而不是索引下溢。
#[test]
fn empty_profile_without_sample_type_returns_empty_results() {
    let profile = Profile::default();

    assert!(parseCPUProfileBySQLLabels(&profile).is_empty());
    assert!(parseCPUProfileForProcess(&profile).is_empty());
}

/// 校验 Start 幂等只注册一次消费者，Stop 幂等并始终注销。
#[test]
#[serial]
fn start_stop_registers_once_and_always_unregisters() {
    cpuprofile::StopCPUProfiler();
    topsql_state::DisableTopSQL();
    assert_eq!(cpuprofile::global_consumers_count(), 0);

    let collector = Arc::new(RecordingCollector::default());
    let mut sql_collector = NewSQLCPUCollector(collector);
    sql_collector.set_collect_interval(Duration::from_millis(5));
    sql_collector.Start();
    sql_collector.Start();
    assert!(sql_collector.is_started());

    topsql_state::EnableTopSQL();
    let deadline = Instant::now() + Duration::from_secs(1);
    while cpuprofile::global_consumers_count() != 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(cpuprofile::global_consumers_count(), 1);

    sql_collector.Stop();
    sql_collector.Stop();
    assert!(!sql_collector.is_started());
    assert_eq!(cpuprofile::global_consumers_count(), 0);
    topsql_state::DisableTopSQL();
}

/// 校验 ProfileContext 标签写入顺序与当前线程标签同步安装。
#[test]
fn context_helpers_preserve_and_install_go_label_values() {
    let context = ProfileContext::default();
    let context = CtxWithSQLDigest(context, "sql-digest".to_owned());
    let context = CtxWithSQLAndPlanDigest(context, "sql-2".to_owned(), "plan-2".to_owned());
    let context = CtxWithProcessInfo(context, 42, 7);

    assert_eq!(context.label(labelSQLDigest), Some("sql-2"));
    assert_eq!(context.label(labelPlanDigest), Some("plan-2"));
    assert_eq!(context.label(labelSQLUID), Some("42_7"));
    assert_eq!(&current_thread_profile_labels(), context.labels());
}
