// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// TopRU 生成用例运行器。
//
// 按 `CaseSpec` 向 `RUWindowAggregator` 注入增量、注册 SQL/Plan 元数据，
// 经 Reporter 上报后断言 RU 记录、执行次数与元数据标记。
// RU（Request Unit）是 TiDB 资源计量单位；digest 是 SQL/执行计划哈希指纹。

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};

use topsql_reporter::datasink::{DataSink, DataSinkError, ReportData as SinkReportData};
use topsql_reporter::reporter::{NewRemoteTopSQLReporter, ReportData};
use topsql_reporter::stmtstats::{
    BinaryDigest, DEFAULT_RU_VERSION, RUIncrement, RUIncrementMap, RUKey,
};
use topsql_reporter::tipb_protobuf::TopRuRecord;
use topsql_reporter::{RUWindowAggregator, alignToInterval, ruReportWindowSeconds};

/// 单条 TopRU 生成用例规格。
#[derive(Clone, Debug)]
pub(crate) struct CaseSpec {
    /// 用例目标 ID，决定注入数据与额外断言分支。
    pub goal_id: &'static str,
    /// 重要级别标记（如 must/should）。
    pub level: &'static str,
    /// 人类可读描述。
    pub description: &'static str,
    /// 是否要求 Reporter 必须发出载荷。
    pub require_send: bool,
    /// RU 记录条数下限。
    pub ru_records_min: usize,
    /// 单条目执行次数下限。
    pub exec_count_min: u64,
    /// 执行次数合计下限。
    pub exec_count_sum_min: u64,
    /// 单条目 total_ru 下限。
    pub total_ru_min: f64,
    /// SQLMeta 归一化文本须包含的标记；空表示不检查。
    pub sql_meta_match_marker: &'static str,
    /// 是否要求存在 PlanMetas。
    pub plan_meta_required: Option<bool>,
}

/// 将 Reporter 发出的数据转发到同步通道，供用例断言。
struct TopRuCaseSink {
    tx: SyncSender<ReportData>,
}

impl DataSink for TopRuCaseSink {
    fn try_send(&self, data: Arc<SinkReportData>, _deadline: Instant) -> Result<(), DataSinkError> {
        self.tx
            .send(ReportData::from_sink_report_data(&data))
            .map_err(|_| DataSinkError::Closed)
    }

    fn on_reporter_closing(&self) {}
}

/// 构造 RU 聚合键。
fn ru_key(user: &str, sql_digest: &[u8], plan_digest: &[u8]) -> RUKey {
    RUKey {
        User: user.to_owned(),
        SQLDigest: BinaryDigest(sql_digest.to_vec()),
        PlanDigest: BinaryDigest(plan_digest.to_vec()),
    }
}

/// 构造 RU 增量。
fn increment(total_ru: f64, exec_count: u64, exec_duration: u64) -> RUIncrement {
    RUIncrement {
        TotalRU: total_ru,
        ExecCount: exec_count,
        ExecDuration: exec_duration,
    }
}

/// 向 Reporter 注册 SQL/Plan 元数据，供随后从真实缓存取出并 doReport。
fn register_metadata(
    reporter: &topsql_reporter::reporter::RemoteTopSQLReporter,
    sql_digest: &[u8],
    plan_digest: &[u8],
    normalized_sql: String,
    normalized_plan: String,
) {
    reporter.RegisterSQL(sql_digest.to_vec(), normalized_sql.clone(), false);
    reporter.RegisterPlan(plan_digest.to_vec(), normalized_plan.clone(), false);
}

/// 按 CaseSpec 注入数据、触发上报并断言载荷。
pub(crate) fn run_top_ru_case(cs: &CaseSpec) {
    let reporter = NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let (tx, rx) = sync_channel(1);
    reporter.Register(Arc::new(TopRuCaseSink { tx })).unwrap();

    let aggregator = RUWindowAggregator::new();
    let record_count = cs.ru_records_min.max(1);
    let marker = if cs.sql_meta_match_marker.is_empty() {
        format!("topru_gen_{}", cs.goal_id.to_lowercase())
    } else {
        cs.sql_meta_match_marker.to_owned()
    };
    let total_ru_baseline = if cs.total_ru_min <= 0.0 {
        0.001
    } else {
        cs.total_ru_min
    };
    // 将合计执行次数摊到首条，同时满足 min 与 sum 约束。
    let required_sum = (cs.exec_count_sum_min as usize).max(record_count);
    let mut exec_counts = vec![1_u64; record_count];
    exec_counts[0] += (required_sum - record_count) as u64;
    exec_counts[0] = exec_counts[0].max(cs.exec_count_min);

    let sample_ts = 1_700_000_000_u64;
    // 特定 goal 走专用注入；其余按 record_count 生成通用批次。
    match cs.goal_id {
        "key_aggregation_by_user_sql_plan" => {
            let sql = b"S_G7";
            let plan = b"P_G7";
            register_metadata(
                &reporter,
                sql,
                plan,
                format!("/* {marker} */ select 7"),
                format!("plan_{marker}_7"),
            );
            let mut batch = RUIncrementMap::new();
            batch.insert(ru_key("u1", sql, plan), increment(7.0, 1, 1000));
            batch.insert(ru_key("u2", sql, plan), increment(9.0, 1, 1000));
            aggregator.add_batch(sample_ts, batch, DEFAULT_RU_VERSION);
        }
        "same_timestamp_multiple_finish_accumulate" => {
            let sql = b"S_G8";
            let plan = b"P_G8";
            register_metadata(
                &reporter,
                sql,
                plan,
                format!("/* {marker} */ select 8"),
                format!("plan_{marker}_8"),
            );
            aggregator.add_batch(
                sample_ts,
                HashMap::from([(ru_key("root", sql, plan), increment(3.0, 1, 1000))]),
                DEFAULT_RU_VERSION,
            );
            aggregator.add_batch(
                sample_ts,
                HashMap::from([(ru_key("root", sql, plan), increment(4.0, 2, 2000))]),
                DEFAULT_RU_VERSION,
            );
        }
        "internal_sql_empty_user_handling" => {
            let sql = b"S_G10";
            let plan = b"P_G10";
            register_metadata(
                &reporter,
                sql,
                plan,
                format!("/* {marker} */ select 10"),
                format!("plan_{marker}_10"),
            );
            aggregator.add_batch(
                sample_ts,
                HashMap::from([(ru_key("", sql, plan), increment(10.0, 1, 1000))]),
                DEFAULT_RU_VERSION,
            );
        }
        "short_exec_time_lt_1s_handling" => {
            let sql = b"S_G11";
            let plan = b"P_G11";
            register_metadata(
                &reporter,
                sql,
                plan,
                format!("/* {marker} */ select 11"),
                format!("plan_{marker}_11"),
            );
            aggregator.add_batch(
                sample_ts,
                HashMap::from([(
                    ru_key("root", sql, plan),
                    increment(11.0, 1, Duration::from_millis(500).as_nanos() as u64),
                )]),
                DEFAULT_RU_VERSION,
            );
        }
        _ => {
            let mut batch = RUIncrementMap::with_capacity(record_count);
            for index in 0..record_count {
                let sql = format!("S_{}_{}", cs.goal_id, index).into_bytes();
                let plan = format!("P_{}_{}", cs.goal_id, index).into_bytes();
                register_metadata(
                    &reporter,
                    &sql,
                    &plan,
                    format!("/* {marker} */ select {index}"),
                    format!("plan_{marker}_{index}"),
                );
                batch.insert(
                    ru_key("root", &sql, &plan),
                    increment(
                        total_ru_baseline + (index + 1) as f64,
                        exec_counts[index],
                        (1000 + index * 100) as u64,
                    ),
                );
            }
            aggregator.add_batch(sample_ts, batch, DEFAULT_RU_VERSION);
        }
    }

    // 与 Go runner 一致，从 Reporter 注册缓存取得元数据，避免手工构造载荷
    // 掩盖 RegisterSQL/RegisterPlan 或 keyspace/plan 转换路径的缺陷。
    let report_ts = alignToInterval(sample_ts, ruReportWindowSeconds) + ruReportWindowSeconds;
    reporter.BindKeyspaceName(b"topru-gen-keyspace".to_vec());
    reporter.Start();
    reporter.takeDataAndSendToReportChan(report_ts);
    let mut report_data = rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("timeout waiting for reporter metadata: {error}"));
    report_data.RURecords =
        aggregator.take_report_records(report_ts, 60, b"topru-gen-keyspace".to_vec());
    reporter.doReport(&report_data);

    let payload = match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(payload) => payload,
        Err(error) if cs.require_send => {
            panic!(
                "timeout waiting for payload for goal {}: {error}",
                cs.goal_id
            )
        }
        Err(_) => return,
    };
    assert!(!cs.require_send || payload.hasData(), "goal {}", cs.goal_id);
    if cs.ru_records_min > 0 {
        assert!(
            payload.RURecords.len() >= cs.ru_records_min,
            "goal {}",
            cs.goal_id
        );
    }

    let mut max_exec_count = 0_u64;
    let mut sum_exec_count = 0_u64;
    let mut max_total_ru = 0.0_f64;
    for record in &payload.RURecords {
        for item in &record.items {
            max_exec_count = max_exec_count.max(item.exec_count);
            sum_exec_count += item.exec_count;
            max_total_ru = max_total_ru.max(item.total_ru);
        }
    }
    if cs.exec_count_min > 0 {
        assert!(max_exec_count >= cs.exec_count_min, "goal {}", cs.goal_id);
    }
    if cs.exec_count_sum_min > 0 {
        assert!(
            sum_exec_count >= cs.exec_count_sum_min,
            "goal {}",
            cs.goal_id
        );
    }
    if cs.total_ru_min > 0.0 {
        assert!(max_total_ru >= cs.total_ru_min, "goal {}", cs.goal_id);
    }
    if !cs.sql_meta_match_marker.is_empty() {
        assert!(
            payload
                .SQLMetas
                .iter()
                .any(|meta| meta.NormalizedSQL.contains(cs.sql_meta_match_marker)),
            "missing SQLMeta marker: {}",
            cs.sql_meta_match_marker
        );
    }
    if cs.plan_meta_required == Some(true) {
        assert!(!payload.PlanMetas.is_empty(), "goal {}", cs.goal_id);
    }
    assert_top_ru_case_payload(cs.goal_id, &payload);
    reporter.Close();
}

/// 按 goal_id 做额外语义断言（多用户聚合、同时间戳累加等）。
fn assert_top_ru_case_payload(goal_id: &str, payload: &ReportData) {
    match goal_id {
        "key_aggregation_by_user_sql_plan" => {
            let users = payload
                .RURecords
                .iter()
                .filter(|record| record.sql_digest == b"S_G7" && record.plan_digest == b"P_G7")
                .map(|record| record.user.as_str())
                .collect::<HashSet<_>>();
            assert_eq!(users.len(), 2);
            assert!(users.contains("u1"));
            assert!(users.contains("u2"));
        }
        "same_timestamp_multiple_finish_accumulate" => {
            let record = find_ru_record_by_digest(&payload.RURecords, "root", b"S_G8", b"P_G8")
                .expect("S_G8 record");
            assert_eq!(record.items.len(), 1);
            assert!((record.items[0].total_ru - 7.0).abs() <= 1e-9);
            assert_eq!(record.items[0].exec_count, 3);
            assert_eq!(record.items[0].exec_duration, 3000);
        }
        "internal_sql_empty_user_handling" => {
            let record = find_ru_record_by_digest(&payload.RURecords, "", b"S_G10", b"P_G10")
                .expect("S_G10 record");
            assert!(!record.items.is_empty());
        }
        "short_exec_time_lt_1s_handling" => {
            let record = find_ru_record_by_digest(&payload.RURecords, "root", b"S_G11", b"P_G11")
                .expect("S_G11 record");
            assert!(!record.items.is_empty());
            assert_eq!(
                record.items[0].exec_duration,
                Duration::from_millis(500).as_nanos() as u64
            );
        }
        _ => {}
    }
}

/// 按用户与 digest 查找 RU 记录。
fn find_ru_record_by_digest<'a>(
    records: &'a [TopRuRecord],
    user: &str,
    sql_digest: &[u8],
    plan_digest: &[u8],
) -> Option<&'a TopRuRecord> {
    records.iter().find(|record| {
        record.user == user && record.sql_digest == sql_digest && record.plan_digest == plan_digest
    })
}
