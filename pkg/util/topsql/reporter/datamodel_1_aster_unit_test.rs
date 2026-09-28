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

// TopSQL reporter 数据模型迁移基线单元测试。
//
// 覆盖 Record 按时间戳合并与 tipb 导出、Collecting 任意 digest 字节与空 plan 归并、
// NormalizedSql/PlanMap 首次写入与容量限制、DataSinkRegisterer 幂等/容量与 TopSQL/TopRU
// 开关，以及 PubSub 订阅解析、发送顺序与 ChannelFull/取消语义。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::datamodel::{
    Collecting, KvStatementStatsItem, NormalizedPlanMap, NormalizedSqlMap, Record,
    StatementStatsItem,
};
use crate::datasink::{
    DataSink, DataSinkError, DataSinkRegisterer, DefaultDataSinkRegisterer, ReportData,
};
use crate::pubsub::{
    PubSubDataSink, PubSubResponse, PubSubStream, parse_top_ru_subscription,
    parse_top_sql_subscription,
};
use protobuf::Message;
use serial_test::serial;

/// 构造带单目标 KV 执行次数的 StatementStatsItem 测试数据。
fn statement_stats(exec_count: u64, target: &str, kv_count: u64) -> StatementStatsItem {
    StatementStatsItem {
        KvStatsItem: KvStatementStatsItem {
            KvExecCount: Some([(target.to_owned(), kv_count)].into_iter().collect()),
        },
        ExecCount: exec_count,
        SumDurationNs: exec_count * 10,
        DurationCount: exec_count,
        NetworkInBytes: exec_count * 20,
        NetworkOutBytes: exec_count * 30,
    }
}

/// 校验 Record 按时间戳归并 CPU/语句统计，且 to_proto 字段与 Go 一致。
#[test]
fn record_merge_and_proto_match_go_behavior() {
    let mut first = Record::new(b"sql".to_vec(), b"plan".to_vec());
    first.append_cpu_time(3, 3);
    first.append_cpu_time(1, 1);
    first.append_stmt_stats_item(1, statement_stats(2, "tikv-1", 4));

    let mut second = Record::new(b"sql".to_vec(), b"plan".to_vec());
    second.append_cpu_time(2, 2);
    second.append_cpu_time(1, 5);
    second.append_stmt_stats_item(1, statement_stats(7, "tikv-1", 8));

    first.merge(Some(&mut second));
    assert_eq!(first.timestamps(), vec![1, 2, 3]);
    assert_eq!(first.cpu_times_ms(), vec![6, 2, 3]);
    assert_eq!(first.total_cpu_time_ms(), 11);
    assert_eq!(first.statement_stats()[0].ExecCount, 9);
    assert_eq!(
        first.statement_stats()[0]
            .KvStatsItem
            .KvExecCount
            .as_ref()
            .unwrap()["tikv-1"],
        12
    );

    let proto = first.to_proto(b"keyspace".to_vec());
    assert_eq!(proto.get_keyspace_name(), b"keyspace");
    assert_eq!(proto.get_sql_digest(), b"sql");
    assert_eq!(proto.get_plan_digest(), b"plan");
    assert_eq!(proto.get_items().len(), 3);
    assert_eq!(proto.get_items()[0].get_timestamp_sec(), 1);
    assert_eq!(proto.get_items()[0].get_cpu_time_ms(), 6);
    assert_eq!(proto.get_items()[0].get_stmt_exec_count(), 9);
}

/// 校验 Collecting 保留非法 UTF-8 digest，并将空 plan 记录并入唯一有效 plan。
#[test]
fn collecting_preserves_arbitrary_digest_bytes_and_merges_only_one_valid_plan() {
    let mut collecting = Collecting::new();

    // Both byte sequences are invalid UTF-8 and collapse to U+FFFD under
    // from_utf8_lossy. Go map keys preserve the original bytes.
    // 两段均为非法 UTF-8；Go map 键保留原始字节，故不能用有损 UTF-8 转换。
    collecting
        .get_or_create_record(&[0xff], b"p")
        .append_cpu_time(1, 1);
    collecting
        .get_or_create_record(&[0xfe], b"p")
        .append_cpu_time(1, 2);
    assert_eq!(collecting.record_count(), 2);

    collecting
        .get_or_create_record(b"sql", b"")
        .append_cpu_time(1, 3);
    collecting
        .get_or_create_record(b"sql", b"valid")
        .append_cpu_time(1, 4);
    collecting.append_others_cpu_time(1, 9);

    let records = collecting.report_records();
    assert_eq!(records.last().unwrap().sql_digest(), b"");
    let merged = records
        .iter()
        .find(|record| record.sql_digest() == b"sql")
        .unwrap();
    assert_eq!(merged.plan_digest(), b"valid");
    assert_eq!(merged.cpu_times_ms(), vec![7]);
}

/// 校验 SQL/Plan 元数据表：保留首值、take 清空、容量上限，以及 plan 解码失败跳过。
#[test]
fn normalized_meta_maps_keep_first_value_take_atomically_and_handle_plan_errors() {
    let sql = NormalizedSqlMap::new(2);
    assert!(sql.register(b"s1", "select 1".into(), true));
    assert!(!sql.register(b"s1", "replacement".into(), false));
    assert!(sql.register(b"s2", "select 2".into(), false));
    assert!(!sql.register(b"s3", "select 3".into(), false));

    let snapshot = sql.take();
    assert_eq!(sql.len(), 0);
    let mut metas = snapshot.to_proto(b"ks".to_vec());
    metas.sort_by(|a, b| a.get_sql_digest().cmp(b.get_sql_digest()));
    assert_eq!(metas.len(), 2);
    assert_eq!(metas[0].get_normalized_sql(), "select 1");
    assert!(metas[0].get_is_internal_sql());

    let plans = NormalizedPlanMap::new(4);
    assert!(plans.register(b"small", "binary-small".into(), false));
    assert!(plans.register(b"large", "binary-large".into(), true));
    assert!(plans.register(b"bad", "bad".into(), false));
    let mut metas = plans.to_proto(
        b"ks".to_vec(),
        |plan| {
            if plan == "bad" {
                Err("decode failed".to_owned())
            } else {
                Ok(format!("decoded:{plan}"))
            }
        },
        |plan| format!("compressed:{}", String::from_utf8_lossy(plan)),
    );
    metas.sort_by(|a, b| a.get_plan_digest().cmp(b.get_plan_digest()));
    assert_eq!(metas.len(), 2);
    assert!(
        metas
            .iter()
            .any(|meta| meta.get_normalized_plan() == "decoded:binary-small")
    );
    assert!(
        metas
            .iter()
            .any(|meta| meta.get_encoded_normalized_plan() == "compressed:binary-large")
    );
}

/// 空 DataSink：try_send 恒成功，用于注册器行为测试。
#[derive(Default)]
struct MockSink;

impl DataSink for MockSink {
    fn try_send(&self, _data: Arc<ReportData>, _deadline: Instant) -> Result<(), DataSinkError> {
        Ok(())
    }

    fn on_reporter_closing(&self) {}
}

/// 校验注册器幂等、上限、关闭后拒绝，以及 TopSQL/TopRU 开关与间隔联动。
#[test]
#[serial]
fn registerer_is_idempotent_bounded_and_tracks_top_sql() {
    crate::topsql_state::DisableTopSQL();
    while crate::topsql_state::TopRUEnabled() {
        crate::topsql_state::DisableTopRU();
    }

    let registerer = DefaultDataSinkRegisterer::new();
    let sink: Arc<dyn DataSink> = Arc::new(MockSink);
    registerer.register(sink.clone()).unwrap();
    registerer.register(sink.clone()).unwrap();
    assert_eq!(registerer.sink_count(), 1);
    assert!(crate::topsql_state::TopSQLEnabled());

    for _ in 1..10 {
        registerer.register(Arc::new(MockSink)).unwrap();
    }
    assert_eq!(
        registerer.register(Arc::new(MockSink)).unwrap_err(),
        DataSinkError::TooManyDataSinks
    );

    registerer.deregister(&sink);
    registerer.close();
    assert_eq!(
        registerer.register(Arc::new(MockSink)).unwrap_err(),
        DataSinkError::RegistererClosed
    );
    assert!(!crate::topsql_state::TopSQLEnabled());

    let top_ru_registerer = DefaultDataSinkRegisterer::new();
    let invalid: Arc<dyn DataSink> = Arc::new(PubSubDataSink::new(
        Arc::new(MockStream::default()),
        false,
        true,
        99,
    ));
    assert_eq!(
        top_ru_registerer.register(invalid).unwrap_err(),
        DataSinkError::InvalidTopRuInterval(99)
    );
    assert_eq!(top_ru_registerer.sink_count(), 0);

    let top_ru_only: Arc<dyn DataSink> = Arc::new(PubSubDataSink::new(
        Arc::new(MockStream::default()),
        false,
        true,
        15,
    ));
    top_ru_registerer.register(top_ru_only.clone()).unwrap();
    assert!(crate::topsql_state::TopRUEnabled());
    assert!(!crate::topsql_state::TopSQLEnabled());
    assert_eq!(crate::topsql_state::GetTopRUItemInterval(), 15);
    top_ru_registerer.deregister(&top_ru_only);
    assert!(!crate::topsql_state::TopRUEnabled());
    assert_eq!(
        crate::topsql_state::GetTopRUItemInterval(),
        crate::topsql_state::DefTiDBTopRUItemIntervalSeconds
    );
}

/// 记录 PubSub 发送消息类型标签的 mock 流。
#[derive(Default)]
struct MockStream {
    responses: Mutex<Vec<&'static str>>,
}

impl PubSubStream for MockStream {
    fn send(&self, response: PubSubResponse) -> Result<(), DataSinkError> {
        let kind = match response {
            PubSubResponse::TopSqlRecord(_) => "record",
            PubSubResponse::TopRuRecord(_) => "ru_record",
            PubSubResponse::SqlMeta(_) => "sql_meta",
            PubSubResponse::PlanMeta(_) => "plan_meta",
        };
        self.responses.lock().unwrap().push(kind);
        Ok(())
    }
}

/// 校验订阅解析、发送顺序、ChannelFull、取消关闭与 report ticker 间隔语义。
#[test]
#[serial]
fn subscription_parsing_send_order_channel_full_and_ticker_match_go() {
    let mut request = tipb::TopSqlSubRequest::new();
    assert!(parse_top_sql_subscription(None));
    assert!(parse_top_sql_subscription(Some(&request)));
    assert_eq!(
        parse_top_ru_subscription(Some(&request)).unwrap(),
        (false, 0)
    );

    request
        .mut_collectors()
        .push(tipb::CollectorType::CollectorTypeTopru);
    assert!(!parse_top_sql_subscription(Some(&request)));
    assert_eq!(
        parse_top_ru_subscription(Some(&request)).unwrap_err(),
        DataSinkError::TopRuConfigEmpty
    );
    let mut config = tipb::TopRuConfig::new();
    config.set_item_interval_seconds(tipb::ItemInterval::ItemInterval15s);
    request.set_topru(config);
    assert_eq!(
        parse_top_ru_subscription(Some(&request)).unwrap(),
        (true, 15)
    );

    crate::topsql_state::EnableTopRU();
    let stream = Arc::new(MockStream::default());
    let sink = PubSubDataSink::new(stream.clone(), true, true, 15);
    let mut record = tipb::TopSqlRecord::new();
    record.set_sql_digest(b"sql".to_vec());
    let mut ru_record = tipb::TopRuRecord::new();
    ru_record.set_user("user".to_owned());
    let mut sql_meta = tipb::SqlMeta::new();
    sql_meta.set_sql_digest(b"sql".to_vec());
    let mut plan_meta = tipb::PlanMeta::new();
    plan_meta.set_plan_digest(b"plan".to_vec());
    let data = Arc::new(ReportData {
        data_records: vec![record],
        ru_records: vec![ru_record],
        sql_metas: vec![sql_meta],
        plan_metas: vec![plan_meta],
    });

    // 期望发送顺序：TopSqlRecord → TopRuRecord → SqlMeta → PlanMeta。
    sink.do_send(data.as_ref()).unwrap();
    assert_eq!(
        *stream.responses.lock().unwrap(),
        vec!["record", "ru_record", "sql_meta", "plan_meta"]
    );
    // 通道容量为 1：第二次 try_send 应返回 ChannelFull。
    sink.try_send(data.clone(), Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        sink.try_send(data, Instant::now() + Duration::from_secs(1))
            .unwrap_err(),
        DataSinkError::ChannelFull
    );

    let cancellable = Arc::new(PubSubDataSink::new(
        Arc::new(MockStream::default()),
        true,
        false,
        0,
    ));
    let worker_sink = cancellable.clone();
    let worker = std::thread::spawn(move || worker_sink.run());
    cancellable.cancel();
    assert_eq!(worker.join().unwrap().unwrap_err(), DataSinkError::Closed);
    crate::topsql_state::DisableTopRU();

    let restore = crate::report_ticker::set_report_ticker_interval_seconds_for_test(1);
    let ticker = crate::report_ticker::new_report_ticker();
    assert_eq!(ticker.interval(), Duration::from_secs(1));
    assert!(ticker.recv_timeout(Duration::from_millis(1200)).is_ok());
    restore();
}
