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

// Reporter / RU 数据模型的 AsterSQL 迁移补充单元测试。
//
// 相对 Go 原测额外覆盖：同时间戳 RU 合并、pre-cap/compaction、稀疏桶重分组、
// 迟到批次与 version handover、网络阈值 K 大规则、并发写与通道满丢弃计数。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::*;

/// 仅用于验证公开 PubSub DataSink 能直接注册到 RemoteTopSQLReporter。
struct NoopPubSubStream;

impl pubsub::PubSubStream for NoopPubSubStream {
    fn send(&self, _response: pubsub::PubSubResponse) -> Result<(), datasink::DataSinkError> {
        Ok(())
    }
}

/// 记录 PubSub 收到的响应，供统一 fan-out 端到端断言。
#[derive(Default)]
struct RecordingPubSubStream {
    responses: Mutex<Vec<pubsub::PubSubResponse>>,
}

impl pubsub::PubSubStream for RecordingPubSubStream {
    fn send(&self, response: pubsub::PubSubResponse) -> Result<(), datasink::DataSinkError> {
        self.responses.lock().unwrap().push(response);
        Ok(())
    }
}

/// 字符串转 BinaryDigest。
fn digest(value: &str) -> BinaryDigest {
    BinaryDigest(value.as_bytes().to_vec())
}

/// 构造 ExecCount=1、ExecDuration=10 的 RU 增量。
fn increment(total_ru: f64) -> RUIncrement {
    RUIncrement {
        TotalRU: total_ru,
        ExecCount: 1,
        ExecDuration: 10,
    }
}

/// 单用户/单 SQL 的一批 RU 增量。
fn one_batch(user: &str, sql: &str, total_ru: f64) -> RUIncrementMap {
    HashMap::from([(
        RUKey {
            User: user.to_owned(),
            SQLDigest: digest(sql),
            PlanDigest: digest("plan"),
        },
        increment(total_ru),
    )])
}

#[test]
/// 同 timestamp 多次 add 合并到同一 item，对齐 Go。
fn ru_record_merges_same_timestamp_like_go() {
    let mut record = newRURecord(digest("sql"), digest("plan"));
    record.add(1000, 10.0, 1, 100);
    record.add(1000, 5.0, 2, 50);
    record.add(1001, 20.0, 1, 200);

    assert_eq!(record.items.len(), 2);
    assert_eq!(record.totalRU, 35.0);
    assert_eq!(record.items[0].totalRU, 15.0);
    assert_eq!(record.items[0].execCount, 3);
    assert_eq!(record.items[0].execDuration, 150);
}

#[test]
/// 超 cap 落入 others；compact 后 RU 总量不变且 others 分桶可区分。
fn pre_caps_and_compaction_preserve_ru_in_distinct_others_buckets() {
    let mut collecting = newRUCollectingWithCaps(1, 1);
    collecting.add(
        1,
        RUKey::new("app@host", b"hot", b"plan"),
        Some(&increment(10.0)),
    );
    collecting.add(
        2,
        RUKey::new("app@host", b"overflow", b"plan"),
        Some(&increment(8.0)),
    );
    collecting.add(
        3,
        RUKey::new("other@host", b"sql", b"plan"),
        Some(&increment(7.0)),
    );

    assert_eq!(collecting.users.len(), 1);
    assert_eq!(
        collecting.users["app@host"]
            .othersRec
            .as_ref()
            .unwrap()
            .totalRU,
        8.0
    );
    assert_eq!(collecting.othersUser.as_ref().unwrap().totalRU, 7.0);

    let mut compacted = collecting.compactWithLimits(1, 1).unwrap();
    let records = compacted.toTopRURecords(b"ks".to_vec());
    assert_eq!(records.len(), 3);
    assert!(
        records
            .iter()
            .any(|record| record.get_user() == "app@host" && record.get_sql_digest().is_empty())
    );
    assert!(
        records
            .iter()
            .any(|record| record.get_user() == othersUserWireLabel)
    );
    let total: f64 = records
        .iter()
        .flat_map(|record| record.get_items())
        .map(|item| item.get_total_ru())
        .sum();
    assert_eq!(total, 25.0);
}

#[test]
/// 稀疏时间点重分组不产生空洞虚假点。
fn sparse_buckets_regroup_without_phantom_points() {
    let aggregator = RUWindowAggregator::new();
    aggregator.add_batch(1, one_batch("u", "sql", 2.0), 1);
    aggregator.add_batch(31, one_batch("u", "sql", 3.0), 1);

    let records = aggregator.take_report_records(60, 30, b"ks".to_vec());
    assert_eq!(records.len(), 1);
    let items = records[0].get_items();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0].get_timestamp_sec(), items[0].get_total_ru()),
        (0, 2.0)
    );
    assert_eq!(
        (items[1].get_timestamp_sec(), items[1].get_total_ru()),
        (30, 3.0)
    );
}

#[test]
/// 迟到批次前移；handover 后旧窗口数据丢弃。
fn late_batches_shift_forward_and_version_handover_drops_old_window() {
    let aggregator = RUWindowAggregator::new();
    aggregator.add_batch(1, one_batch("u", "first", 1.0), 1);
    assert_eq!(
        aggregator.take_report_records(60, 60, b"ks".to_vec()).len(),
        1
    );

    aggregator.add_batch(10, one_batch("u", "late", 9.0), 1);
    let shifted = aggregator.take_report_records(120, 60, b"ks".to_vec());
    assert_eq!(shifted.len(), 1);
    assert_eq!(shifted[0].get_items()[0].get_timestamp_sec(), 60);

    aggregator.reset_for_handover(2, 121);
    aggregator.add_batch(122, one_batch("u", "dropped", 100.0), 2);
    aggregator.add_batch(181, one_batch("u", "accepted", 4.0), 2);
    let records = aggregator.take_report_records(240, 60, b"ks".to_vec());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_sql_digest(), b"accepted");
    assert_eq!(records[0].get_items()[0].get_total_ru(), 4.0);
}

#[test]
/// 网络字节阈值取第 K 大，与 Go 一致。
fn network_threshold_matches_go_kth_largest_rule() {
    let mut stats = StatementStatsMap::new();
    for (index, bytes) in [10_u64, 3, 7, 1].into_iter().enumerate() {
        stats.insert(
            SQLPlanDigest::new(format!("s{index}").as_bytes(), b"p"),
            StatementStatsItem {
                NetworkInBytes: bytes,
                ..Default::default()
            },
        );
    }
    let mut scratch = Vec::new();
    assert_eq!(findKthNetworkBytes(&stats, 2, &mut scratch), 7);
    assert_eq!(findKthNetworkBytes(&stats, 4, &mut scratch), 0);
}

#[test]
/// 多线程并发 add_batch 时 RU 总量守恒。
fn concurrent_writers_preserve_all_ru_in_the_bucket() {
    let aggregator = Arc::new(RUWindowAggregator::new());
    let mut writers = Vec::new();
    for _ in 0..8 {
        let aggregator = aggregator.clone();
        writers.push(std::thread::spawn(move || {
            for _ in 0..100 {
                aggregator.add_batch(1, one_batch("u", "sql", 1.0), 1);
            }
        }));
    }
    for writer in writers {
        writer.join().unwrap();
    }
    let records = aggregator.take_report_records(60, 60, b"ks".to_vec());
    let total: f64 = records[0]
        .get_items()
        .iter()
        .map(|item| item.get_total_ru())
        .sum();
    assert_eq!(total, 800.0);
}

#[test]
/// 压缩后迟到目标被丢弃并计入 dropped 指标。
fn compacted_late_target_is_dropped_and_accounted() {
    let aggregator = RUWindowAggregator::new();
    aggregator.add_batch(1, one_batch("u", "first", 1.0), 1);
    let _ = aggregator.take_report_records(60, 60, b"ks".to_vec());
    aggregator.add_batch(61, one_batch("u", "current", 1.0), 1);
    aggregator.add_batch(76, one_batch("u", "rotate", 1.0), 1);
    aggregator.add_batch(10, one_batch("u", "late", 999.0), 1);
    assert_eq!(aggregator.dropped_late_keys(), 1);
    assert_eq!(aggregator.dropped_late_ru(), 999.0);
}

#[test]
#[serial_test::serial]
/// 收集/上报通道满时非阻塞丢弃，channelDropCounts 可观测。
fn reporter_channels_drop_without_blocking_when_full() {
    let reporter = NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let cpu = vec![collector::SQLCPUTimeRecord {
        SQLDigest: b"sql".to_vec(),
        PlanDigest: b"plan".to_vec(),
        CPUTimeMs: 1,
    }];
    reporter.Collect(cpu.clone());
    reporter.Collect(cpu.clone());
    reporter.Collect(cpu);

    let ru = one_batch("u", "sql", 1.0);
    reporter.CollectRUIncrements(ru.clone(), 1);
    reporter.CollectRUIncrements(ru.clone(), 1);
    reporter.CollectRUIncrements(ru, 1);

    reporter.takeDataAndSendToReportChan(60);
    reporter.takeDataAndSendToReportChan(120);
    assert_eq!(reporter.channelDropCounts(), (1, 0, 1, 1));
    reporter.Close();
}

#[test]
#[serial_test::serial]
/// Go 的 RemoteTopSQLReporter 实现 DataSinkRegisterer，必须接受同包 PubSub sink。
fn remote_reporter_registers_the_public_pubsub_sink() {
    let reporter = NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let sink: Arc<dyn datasink::DataSink> = Arc::new(pubsub::PubSubDataSink::new(
        Arc::new(NoopPubSubStream),
        true,
        false,
        0,
    ));
    reporter.Register(sink).unwrap();
    reporter.Close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
/// 同一 reporter 同时向真实 PubSub 与 SingleTarget fan-out 同一 protobuf 载荷。
async fn remote_reporter_fans_out_to_pubsub_and_single_target() {
    let mut server = topsql_protocol::server::StartMockAgentServer()
        .await
        .expect("start mock agent server");
    let reporter = NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));

    let stream = Arc::new(RecordingPubSubStream::default());
    let pubsub_sink = Arc::new(pubsub::PubSubDataSink::new(stream.clone(), true, false, 0));
    reporter
        .Register(pubsub_sink.clone() as Arc<dyn datasink::DataSink>)
        .unwrap();

    let address = Arc::new(single_target::MutableReceiverAddress::new(server.Address()));
    let single_target = single_target::NewSingleTargetDataSinkWithReceiver(
        reporter.clone() as Arc<dyn datasink::DataSinkRegisterer>,
        address,
    );
    single_target.SetPollInterval(Duration::from_millis(10));
    single_target.Start();

    let mut record = tipb_protobuf::TopSqlRecord::new();
    record.set_sql_digest(b"shared-sql".to_vec());
    record.set_plan_digest(b"shared-plan".to_vec());
    let mut sql_meta = tipb_protobuf::SqlMeta::new();
    sql_meta.set_sql_digest(b"shared-sql".to_vec());
    sql_meta.set_normalized_sql("select 1".to_owned());
    let data = Arc::new(datasink::ReportData {
        data_records: vec![record],
        sql_metas: vec![sql_meta],
        ..Default::default()
    });

    reporter
        .trySend(data, Instant::now() + Duration::from_secs(5))
        .unwrap();
    assert!(pubsub_sink.run_one().unwrap());
    server.WaitCollectCnt(0, 1, Duration::from_secs(5));
    server.WaitCollectCntOfSQLMeta(0, 1, Duration::from_secs(5));

    let responses = stream.responses.lock().unwrap();
    assert!(responses.iter().any(|response| matches!(
        response,
        pubsub::PubSubResponse::TopSqlRecord(record)
            if record.get_sql_digest() == b"shared-sql"
    )));
    assert!(responses.iter().any(|response| matches!(
        response,
        pubsub::PubSubResponse::SqlMeta(meta)
            if meta.get_normalized_sql() == "select 1"
    )));
    drop(responses);
    assert_eq!(
        server.GetLatestRecords().unwrap()[0].sql_digest,
        b"shared-sql"
    );
    assert_eq!(server.GetTotalSQLMetas()[0].normalized_sql, "select 1");

    single_target.Close();
    reporter.Close();
    server.Stop();
}
