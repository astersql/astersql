// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// PubSub DataSink / 订阅解析的单元测试。
//
// 覆盖：发送顺序、TopSQL/TopRU 门控、订阅配置解析、注册失败与无效间隔、
// 以及取消/发送失败时的错误传播。部分用例用 `serial` 隔离全局 topsql 状态。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::datasink::{
    DataSink, DataSinkError, DataSinkRegisterer, DefaultDataSinkRegisterer, ReportData,
};
use crate::pubsub::*;
use serial_test::serial;

/// 记录发送顺序与成功项的 mock 流；可在第 N 次发送失败或回调。
#[derive(Default)]
struct MockStream {
    order: Mutex<Vec<&'static str>>,
    accepted: Mutex<Vec<&'static str>>,
    send_count: AtomicUsize,
    fail_at: AtomicUsize,
    on_send: Mutex<Option<Box<dyn Fn(usize) + Send + Sync>>>,
}

impl MockStream {
    /// 构造在第 `send` 次（1-based）发送时失败的流。
    fn fail_at(send: usize) -> Self {
        Self {
            fail_at: AtomicUsize::new(send),
            ..Default::default()
        }
    }

    /// 返回全部尝试发送的类型顺序。
    fn order(&self) -> Vec<&'static str> {
        self.order.lock().unwrap().clone()
    }

    /// 返回成功接受的类型顺序。
    fn accepted(&self) -> Vec<&'static str> {
        self.accepted.lock().unwrap().clone()
    }

    /// 设置每次发送前的回调（参数为累计发送次数）。
    fn set_on_send(&self, callback: impl Fn(usize) + Send + Sync + 'static) {
        *self.on_send.lock().unwrap() = Some(Box::new(callback));
    }
}

impl PubSubStream for MockStream {
    fn send(&self, response: PubSubResponse) -> Result<(), DataSinkError> {
        let kind = match response {
            PubSubResponse::TopSqlRecord(_) => "record",
            PubSubResponse::TopRuRecord(_) => "ru_record",
            PubSubResponse::SqlMeta(_) => "sql_meta",
            PubSubResponse::PlanMeta(_) => "plan_meta",
        };
        let count = self.send_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.order.lock().unwrap().push(kind);
        if let Some(callback) = self.on_send.lock().unwrap().as_ref() {
            callback(count);
        }
        if self.fail_at.load(Ordering::SeqCst) == count {
            return Err(DataSinkError::Stream("stream send failed".into()));
        }
        self.accepted.lock().unwrap().push(kind);
        Ok(())
    }
}

/// 注册始终失败的 registerer。
struct ErrorRegisterer;

impl DataSinkRegisterer for ErrorRegisterer {
    fn register(&self, _data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        Err(DataSinkError::Stream("register failed".into()))
    }

    fn deregister(&self, _data_sink: &Arc<dyn DataSink>) {}
}

/// 注册时立即排队一份报告，并记录注销是否执行。
struct EnqueueingRegisterer {
    deregistered: AtomicUsize,
}

impl DataSinkRegisterer for EnqueueingRegisterer {
    fn register(&self, data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        data_sink.try_send(
            Arc::new(report_data()),
            Instant::now() + Duration::from_secs(1),
        )
    }

    fn deregister(&self, _data_sink: &Arc<dyn DataSink>) {
        self.deregistered.fetch_add(1, Ordering::SeqCst);
    }
}

/// 模拟底层 gRPC Send 发生 panic。
struct PanickingStream;

impl PubSubStream for PanickingStream {
    fn send(&self, _response: PubSubResponse) -> Result<(), DataSinkError> {
        panic!("stream send panicked")
    }
}

/// 关闭 TopRU/TopSQL 并重置 TopRU 间隔，避免用例间污染。
fn reset_top_sql_state() {
    while crate::topsqlstate::TopRUEnabled() {
        crate::topsqlstate::DisableTopRU();
    }
    crate::topsqlstate::DisableTopSQL();
    crate::topsqlstate::ResetTopRUItemInterval();
}

/// RAII：构造/析构时重置全局 topsql 状态。
struct StateGuard;

impl StateGuard {
    fn new() -> Self {
        reset_top_sql_state();
        Self
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        reset_top_sql_state();
    }
}

/// 构造含 TopRU（可选 TopSQL）collector 的订阅请求。
fn top_ru_request(include_top_sql: bool, interval: tipb::ItemInterval) -> tipb::TopSqlSubRequest {
    let mut request = tipb::TopSqlSubRequest::new();
    if include_top_sql {
        request
            .mut_collectors()
            .push(tipb::CollectorType::CollectorTypeTopsql);
    }
    request
        .mut_collectors()
        .push(tipb::CollectorType::CollectorTypeTopru);
    let mut config = tipb::TopRuConfig::new();
    config.set_item_interval_seconds(interval);
    request.set_topru(config);
    request
}

/// 构造最小 TopSqlRecord fixture。
fn mock_top_sql_record(sql: &[u8], plan: &[u8]) -> tipb::TopSqlRecord {
    let mut record = tipb::TopSqlRecord::new();
    record.set_sql_digest(sql.to_vec());
    record.set_plan_digest(plan.to_vec());
    let mut item = tipb::TopSqlRecordItem::new();
    item.set_timestamp_sec(1);
    item.set_cpu_time_ms(1);
    item.set_stmt_exec_count(1);
    item.set_stmt_kv_exec_count([(String::new(), 1)].into_iter().collect());
    item.set_stmt_duration_sum_ns(1);
    record.mut_items().push(item);
    record
}

/// 构造最小 TopRuRecord fixture。
fn mock_top_ru_record(user: &str, sql: &[u8]) -> tipb::TopRuRecord {
    let mut record = tipb::TopRuRecord::new();
    record.set_user(user.into());
    record.set_sql_digest(sql.to_vec());
    let mut item = tipb::TopRuRecordItem::new();
    item.set_timestamp_sec(1);
    item.set_total_ru(1.0);
    item.set_exec_count(1);
    item.set_exec_duration(1);
    record.mut_items().push(item);
    record
}

/// 含四类载荷的标准 ReportData。
fn report_data() -> ReportData {
    let mut sql_meta = tipb::SqlMeta::new();
    sql_meta.set_sql_digest(b"S1".to_vec());
    sql_meta.set_normalized_sql("SQL-1".into());
    let mut plan_meta = tipb::PlanMeta::new();
    plan_meta.set_plan_digest(b"P1".to_vec());
    plan_meta.set_normalized_plan("PLAN-1".into());
    ReportData {
        data_records: vec![mock_top_sql_record(b"S1", b"P1")],
        ru_records: vec![mock_top_ru_record("user1", b"S1")],
        sql_metas: vec![sql_meta],
        plan_metas: vec![plan_meta],
    }
}

#[test]
#[serial]
/// try_send + run_one 发送 TopSQL/meta，关闭后再 run_one 应 Closed。
fn test_pub_sub_data_sink() {
    let _guard = StateGuard::new();
    let stream = Arc::new(MockStream::default());
    let request = tipb::TopSqlSubRequest::new();
    let sink = PubSubDataSink::from_request(Some(&request), stream.clone()).unwrap();
    sink.try_send(
        Arc::new(report_data()),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    assert!(sink.run_one().unwrap());
    assert_eq!(stream.accepted(), vec!["record", "sql_meta", "plan_meta"]);
    sink.on_reporter_closing();
    assert_eq!(sink.run_one().unwrap_err(), DataSinkError::Closed);
}

#[test]
/// 与 Go 的逐条发送后 ctx 检查一致：即使最后一条写出成功，越过截止时间也应报错。
fn test_send_reports_deadline_crossed_during_last_stream_send() {
    let stream = Arc::new(MockStream::default());
    stream.set_on_send(|_| std::thread::sleep(Duration::from_millis(20)));
    let sink = PubSubDataSink::new(stream, true, false, 0);
    sink.try_send(
        Arc::new(ReportData {
            data_records: vec![mock_top_sql_record(b"S1", b"P1")],
            ..Default::default()
        }),
        Instant::now() + Duration::from_millis(5),
    )
    .unwrap();

    assert_eq!(sink.run_one().unwrap_err(), DataSinkError::DeadlineExceeded);
}

#[test]
/// Go run 会恢复发送 panic、注销 sink 并向 Subscribe 返回 nil。
fn test_subscribe_recovers_stream_panic_and_deregisters() {
    let registerer = Arc::new(EnqueueingRegisterer {
        deregistered: AtomicUsize::new(0),
    });
    let service = TopSqlPubSubService::new(registerer.clone());

    assert_eq!(service.subscribe(None, Arc::new(PanickingStream)), Ok(()));
    assert_eq!(registerer.deregistered.load(Ordering::SeqCst), 1);
}

#[test]
/// 非法间隔原样保留在 subscription_config 中（由 registerer 校验）。
fn test_normalize_top_ru_item_interval_invalid() {
    let sink = PubSubDataSink::new(Arc::new(MockStream::default()), true, true, 99);
    assert_eq!(sink.subscription_config().unwrap().item_interval, 99);
}

#[test]
/// 覆盖 TopRU 解析：空请求、缺配置、无 collector、合法间隔。
fn test_parse_top_ru_subscription() {
    assert_eq!(parse_top_ru_subscription(None).unwrap(), (false, 0));

    let mut missing_config = tipb::TopSqlSubRequest::new();
    missing_config
        .mut_collectors()
        .push(tipb::CollectorType::CollectorTypeTopru);
    assert_eq!(
        parse_top_ru_subscription(Some(&missing_config)).unwrap_err(),
        DataSinkError::TopRuConfigEmpty
    );

    let mut no_collector = tipb::TopSqlSubRequest::new();
    no_collector
        .mut_collectors()
        .push(tipb::CollectorType::CollectorTypeTopsql);
    let mut ignored_config = tipb::TopRuConfig::new();
    ignored_config.set_item_interval_seconds(tipb::ItemInterval::ItemInterval30s);
    no_collector.set_topru(ignored_config);
    assert_eq!(
        parse_top_ru_subscription(Some(&no_collector)).unwrap(),
        (false, 0)
    );

    for (interval, seconds) in [
        (tipb::ItemInterval::ItemInterval15s, 15),
        (tipb::ItemInterval::ItemInterval30s, 30),
        (tipb::ItemInterval::ItemInterval60s, 60),
    ] {
        let request = top_ru_request(true, interval);
        assert_eq!(
            parse_top_ru_subscription(Some(&request)).unwrap(),
            (true, seconds)
        );
    }
}

#[test]
/// 覆盖 TopSQL 解析：默认启用、仅 TopRU 时关闭、两者并存时启用。
fn test_parse_top_sql_subscription() {
    assert!(parse_top_sql_subscription(None));
    assert!(parse_top_sql_subscription(Some(
        &tipb::TopSqlSubRequest::new()
    )));

    let mut top_sql = tipb::TopSqlSubRequest::new();
    top_sql
        .mut_collectors()
        .push(tipb::CollectorType::CollectorTypeTopsql);
    assert!(parse_top_sql_subscription(Some(&top_sql)));

    let top_ru_only = top_ru_request(false, tipb::ItemInterval::ItemInterval30s);
    assert!(!parse_top_sql_subscription(Some(&top_ru_only)));
    let both = top_ru_request(true, tipb::ItemInterval::ItemInterval15s);
    assert!(parse_top_sql_subscription(Some(&both)));
}

#[test]
#[serial]
/// TopRU 门控、多订阅者间隔、注册失败/非法间隔与取消半路发送。
fn test_top_ru_pub_sub() {
    let _guard = StateGuard::new();

    crate::topsqlstate::EnableTopRU();
    let stream = Arc::new(MockStream::default());
    let sink = PubSubDataSink::new(stream.clone(), true, true, 15);
    sink.do_send(&ReportData {
        ru_records: vec![mock_top_ru_record("user1", b"S1")],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(stream.accepted(), vec!["ru_record"]);
    crate::topsqlstate::DisableTopRU();

    // 两订阅者独立引用；最新合法间隔全局可见，对齐 Go 服务。
    // Two subscriber configurations keep independent references while the
    // latest valid interval remains globally visible, matching the Go service.
    let registerer = DefaultDataSinkRegisterer::new();
    let first: Arc<dyn DataSink> = Arc::new(PubSubDataSink::new(
        Arc::new(MockStream::default()),
        true,
        true,
        30,
    ));
    let second: Arc<dyn DataSink> = Arc::new(PubSubDataSink::new(
        Arc::new(MockStream::default()),
        true,
        true,
        15,
    ));
    registerer.register(first.clone()).unwrap();
    assert_eq!(crate::topsqlstate::GetTopRUItemInterval(), 30);
    registerer.register(second.clone()).unwrap();
    assert_eq!(crate::topsqlstate::GetTopRUItemInterval(), 15);
    registerer.deregister(&second);
    assert!(crate::topsqlstate::TopRUEnabled());
    assert_eq!(crate::topsqlstate::GetTopRUItemInterval(), 15);
    registerer.deregister(&first);
    assert!(!crate::topsqlstate::TopRUEnabled());

    let service = TopSqlPubSubService::new(Arc::new(ErrorRegisterer));
    let request = top_ru_request(true, tipb::ItemInterval::ItemInterval15s);
    assert!(matches!(
        service.subscribe(Some(&request), Arc::new(MockStream::default())),
        Err(DataSinkError::Stream(message)) if message == "register failed"
    ));
    assert!(!crate::topsqlstate::TopRUEnabled());

    let mut missing = tipb::TopSqlSubRequest::new();
    missing
        .mut_collectors()
        .push(tipb::CollectorType::CollectorTypeTopru);
    let service = TopSqlPubSubService::new(Arc::new(DefaultDataSinkRegisterer::new()));
    assert_eq!(
        service
            .subscribe(Some(&missing), Arc::new(MockStream::default()))
            .unwrap_err(),
        DataSinkError::TopRuConfigEmpty
    );
    assert!(!crate::topsqlstate::TopRUEnabled());
    assert!(!crate::topsqlstate::TopSQLEnabled());

    let invalid: Arc<dyn DataSink> = Arc::new(PubSubDataSink::new(
        Arc::new(MockStream::default()),
        true,
        true,
        99,
    ));
    let registerer = DefaultDataSinkRegisterer::new();
    assert_eq!(
        registerer.register(invalid).unwrap_err(),
        DataSinkError::InvalidTopRuInterval(99)
    );
    assert!(!crate::topsqlstate::TopRUEnabled());
    assert!(!crate::topsqlstate::TopSQLEnabled());

    let top_ru_only: Arc<dyn DataSink> = Arc::new(
        PubSubDataSink::from_request(
            Some(&top_ru_request(false, tipb::ItemInterval::ItemInterval15s)),
            Arc::new(MockStream::default()),
        )
        .unwrap(),
    );
    registerer.register(top_ru_only.clone()).unwrap();
    assert!(crate::topsqlstate::TopRUEnabled());
    assert!(!crate::topsqlstate::TopSQLEnabled());
    assert!(crate::topsqlstate::TopProfilingEnabled());
    registerer.deregister(&top_ru_only);

    let records = vec![
        mock_top_ru_record("user1", b"S1"),
        mock_top_ru_record("user2", b"S2"),
    ];
    crate::topsqlstate::EnableTopRU();
    let stream = Arc::new(MockStream::default());
    PubSubDataSink::new(stream.clone(), true, true, 15)
        .do_send(&ReportData::default())
        .unwrap();
    assert!(stream.accepted().is_empty());

    let stream = Arc::new(MockStream::default());
    PubSubDataSink::new(stream.clone(), true, false, 0)
        .do_send(&ReportData {
            ru_records: records.clone(),
            ..Default::default()
        })
        .unwrap();
    assert!(stream.accepted().is_empty());

    crate::topsqlstate::DisableTopRU();
    let stream = Arc::new(MockStream::default());
    PubSubDataSink::new(stream.clone(), true, true, 15)
        .do_send(&ReportData {
            ru_records: records.clone(),
            ..Default::default()
        })
        .unwrap();
    assert!(stream.accepted().is_empty());

    crate::topsqlstate::EnableTopRU();
    let stream = Arc::new(MockStream::fail_at(1));
    let error = PubSubDataSink::new(stream.clone(), false, true, 15)
        .do_send(&ReportData {
            ru_records: records.clone(),
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(error, DataSinkError::Stream(_)));
    assert!(stream.accepted().is_empty());

    let stream = Arc::new(MockStream::default());
    let sink = Arc::new(PubSubDataSink::new(stream.clone(), false, true, 15));
    let weak_sink = Arc::downgrade(&sink);
    stream.set_on_send(move |count| {
        if count == 1
            && let Some(sink) = weak_sink.upgrade()
        {
            sink.cancel();
        }
    });
    let error = sink
        .do_send(&ReportData {
            ru_records: records,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error, DataSinkError::Closed);
    assert_eq!(stream.accepted(), vec!["ru_record"]);
}

#[test]
/// enable_top_sql=false 时不发送 TopSQL 记录。
fn test_send_top_sql_records_gating() {
    let records = vec![mock_top_sql_record(b"S1", b"P1")];
    let stream = Arc::new(MockStream::default());
    PubSubDataSink::new(stream.clone(), false, false, 0)
        .do_send(&ReportData {
            data_records: records.clone(),
            ..Default::default()
        })
        .unwrap();
    assert!(stream.accepted().is_empty());

    let stream = Arc::new(MockStream::default());
    PubSubDataSink::new(stream.clone(), true, false, 0)
        .do_send(&ReportData {
            data_records: records,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(stream.accepted(), vec!["record"]);
}

#[test]
#[serial]
/// do_send 顺序稳定：records → ru → sql_meta → plan_meta；中途 cancel 截断。
fn test_pub_sub_data_sink_do_send_order_is_stable() {
    let _guard = StateGuard::new();
    crate::topsqlstate::EnableTopRU();
    let mut data = report_data();
    data.data_records.push(mock_top_sql_record(b"S2", b"P2"));
    data.ru_records.push(mock_top_ru_record("user2", b"R2"));

    let stream = Arc::new(MockStream::default());
    PubSubDataSink::new(stream.clone(), true, true, 15)
        .do_send(&data)
        .unwrap();
    assert_eq!(
        stream.order(),
        vec![
            "record",
            "record",
            "ru_record",
            "ru_record",
            "sql_meta",
            "plan_meta"
        ]
    );

    let stream = Arc::new(MockStream::default());
    let sink = Arc::new(PubSubDataSink::new(stream.clone(), true, true, 15));
    let weak_sink = Arc::downgrade(&sink);
    stream.set_on_send(move |count| {
        if count == 3
            && let Some(sink) = weak_sink.upgrade()
        {
            sink.cancel();
        }
    });
    let error = sink.do_send(&data).unwrap_err();
    assert_eq!(error, DataSinkError::Closed);
    assert_eq!(stream.order(), vec!["record", "record", "ru_record"]);
}
