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

// 单目标 DataSink 的 Go 对应单元测试。
//
// 覆盖在启用/未启用 TopRU 时均丢弃 TopRU 记录、转发 TopSQL 与元数据，
// 以及 `sendTopRURecords` 对 Unimplemented 的兼容关闭。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::topsql_state as topsqlstate;
use crate::*;
use serial_test::serial;
use tonic::Code;
use topsql_protocol::server::StartMockAgentServer;

/// 计数型 DataSink 注册器桩。
#[derive(Default)]
struct MockSingleTargetDataSinkRegisterer {
    registrations: Mutex<usize>,
    deregistrations: Mutex<usize>,
}

impl DataSinkRegisterer for MockSingleTargetDataSinkRegisterer {
    fn register(&self, _data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        *self.registrations.lock().unwrap() += 1;
        Ok(())
    }

    fn deregister(&self, _data_sink: &Arc<dyn DataSink>) {
        *self.deregistrations.lock().unwrap() += 1;
    }
}

/// 构造流接口测试用 prost TopRU 记录。
fn mock_top_ru_records() -> Vec<tipb::TopRuRecord> {
    vec![tipb::TopRuRecord {
        sql_digest: b"S1".to_vec(),
        ru: 1.5,
    }]
}

/// 构造统一 DataSink 使用的 protobuf-codec TopSQL 记录。
fn protobuf_top_sql(sql: &[u8], plan: &[u8]) -> tipb_protobuf::TopSqlRecord {
    let mut record = tipb_protobuf::TopSqlRecord::new();
    record.set_sql_digest(sql.to_vec());
    record.set_plan_digest(plan.to_vec());
    record
}

/// 构造统一 DataSink 使用的 protobuf-codec TopRU 记录。
fn protobuf_top_ru(sql: &[u8], ru: f64) -> tipb_protobuf::TopRuRecord {
    let mut record = tipb_protobuf::TopRuRecord::new();
    record.set_sql_digest(sql.to_vec());
    let _ = ru;
    record
}

/// 构造统一 DataSink 使用的 protobuf-codec SQL 元数据。
fn protobuf_sql_meta(sql: &[u8], normalized: &str) -> tipb_protobuf::SqlMeta {
    let mut meta = tipb_protobuf::SqlMeta::new();
    meta.set_sql_digest(sql.to_vec());
    meta.set_normalized_sql(normalized.to_owned());
    meta
}

/// 构造统一 DataSink 使用的 protobuf-codec Plan 元数据。
fn protobuf_plan_meta(plan: &[u8], normalized: &str) -> tipb_protobuf::PlanMeta {
    let mut meta = tipb_protobuf::PlanMeta::new();
    meta.set_plan_digest(plan.to_vec());
    meta.set_normalized_plan(normalized.to_owned());
    meta
}

/// 轮询等待谓词成立，超时则断言失败。
fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(predicate(), "condition not reached before {timeout:?}");
}

/// RAII：构造时关闭 TopRU，析构时再次确保关闭，避免污染其它用例。
struct TopRuStateReset;

impl TopRuStateReset {
    /// 循环 DisableTopRU 直至关闭。
    fn disabled() -> Self {
        while topsqlstate::TopRUEnabled() {
            topsqlstate::DisableTopRU();
        }
        Self
    }
}

impl Drop for TopRuStateReset {
    fn drop(&mut self) {
        while topsqlstate::TopRUEnabled() {
            topsqlstate::DisableTopRU();
        }
    }
}

// Go: TestSingleTargetDataSink.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
/// 对应 Go `TestSingleTargetDataSink`：启用 TopRU 时仍只转发 TopSQL/元数据。
async fn test_single_target_data_sink() {
    let _top_ru_state = TopRuStateReset::disabled();
    topsqlstate::EnableTopRU();
    let mut server = StartMockAgentServer()
        .await
        .expect("start mock agent server");
    let registerer = Arc::new(MockSingleTargetDataSinkRegisterer::default());
    let address = Arc::new(MutableReceiverAddress::new(server.Address()));
    let sink = NewSingleTargetDataSinkWithReceiver(registerer, address);
    sink.SetPollInterval(Duration::from_millis(10));
    sink.Start();

    let records_count = server.RecordsCnt();
    let sql_meta_count = server.SQLMetaCnt();
    let ru_records_count = server.RURecordsCnt();
    sink.TrySend(
        ReportData {
            data_records: vec![protobuf_top_sql(b"S1", b"P1")],
            ru_records: vec![protobuf_top_ru(b"S1", 1.5)],
            sql_metas: vec![protobuf_sql_meta(b"S1", "SQL-1")],
            plan_metas: vec![protobuf_plan_meta(b"P1", "PLAN-1")],
        },
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();

    server.WaitCollectCnt(records_count, 1, Duration::from_secs(5));
    server.WaitCollectCntOfSQLMeta(sql_meta_count, 1, Duration::from_secs(5));
    wait_until(Duration::from_secs(5), || {
        server.GetPlanMetaByDigestBlocking(b"P1", Duration::ZERO).1
    });
    assert_eq!(server.GetLatestRecords().unwrap().len(), 1);
    assert_eq!(server.RURecordsCnt(), ru_records_count);
    assert!(server.GetLatestRURecords().is_none());
    assert!(topsqlstate::TopRUEnabled());
    assert_eq!(server.GetTotalSQLMetas().len(), 1);
    assert_eq!(
        server
            .GetSQLMetaByDigestBlocking(b"S1", Duration::ZERO)
            .0
            .normalized_sql,
        "SQL-1"
    );
    assert_eq!(
        server.GetPlanMetaByDigestBlocking(b"P1", Duration::ZERO).0,
        "PLAN-1"
    );

    sink.Close();
    server.Stop();
}

// Go: TestSingleTargetDataSinkDropsTopRU, covering both TrySend attempts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
/// 对应 Go `TestSingleTargetDataSinkDropsTopRU`：开关 TopRU 两次发送均不落 RU。
async fn test_single_target_data_sink_drops_top_ru() {
    let _top_ru_state = TopRuStateReset::disabled();
    let mut server = StartMockAgentServer()
        .await
        .expect("start mock agent server");
    let registerer = Arc::new(MockSingleTargetDataSinkRegisterer::default());
    let address = Arc::new(MutableReceiverAddress::new(server.Address()));
    let sink = NewSingleTargetDataSinkWithReceiver(registerer, address);
    sink.SetPollInterval(Duration::from_millis(10));
    sink.Start();

    let base_ru_count = server.RURecordsCnt();
    for index in 0..2 {
        if index == 1 {
            topsqlstate::EnableTopRU();
        }
        let old_records = server.RecordsCnt();
        sink.TrySend(
            ReportData {
                data_records: vec![protobuf_top_sql(
                    format!("S{index}").as_bytes(),
                    format!("P{index}").as_bytes(),
                )],
                ru_records: vec![protobuf_top_ru(b"S1", 1.5)],
                ..ReportData::default()
            },
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        server.WaitCollectCnt(old_records, 1, Duration::from_secs(5));
        assert_eq!(server.RURecordsCnt(), base_ru_count);
        assert_eq!(topsqlstate::TopRUEnabled(), index == 1);
    }
    assert!(server.GetLatestRURecords().is_none());

    sink.Close();
    server.Stop();
}

/// 可注入错误的 TopRU 流桩。
struct MockTopRuRecordStream {
    send_error: Option<tonic::Status>,
    close_error: Option<tonic::Status>,
    close_called: bool,
}

impl TopRURecordStream for MockTopRuRecordStream {
    fn Send(&mut self, _record: &tipb::TopRuRecord) -> Result<(), tonic::Status> {
        self.send_error.clone().map_or(Ok(()), Err)
    }

    fn CloseAndRecv(&mut self) -> Result<tipb::EmptyResponse, tonic::Status> {
        self.close_called = true;
        self.close_error
            .clone()
            .map_or_else(|| Ok(tipb::EmptyResponse {}), Err)
    }
}

// Go: TestSendTopRURecordsClosesStreamOnUnimplementedSend.
#[test]
/// 对应 Go：Unimplemented Send 关闭流并返回已发送数；其它 Close 错误上抛。
fn test_send_top_ru_records_closes_stream_on_unimplemented_send() {
    let mut stream = MockTopRuRecordStream {
        send_error: Some(tonic::Status::unimplemented("topru rpc not supported")),
        close_error: Some(tonic::Status::internal("close failed")),
        close_called: false,
    };

    let sent_count = sendTopRURecords(&mut stream, &mock_top_ru_records()).unwrap();
    assert_eq!(sent_count, 0);
    assert!(stream.close_called);

    let mut stream = MockTopRuRecordStream {
        send_error: None,
        close_error: Some(tonic::Status::internal("close failed")),
        close_called: false,
    };
    let error = sendTopRURecords(&mut stream, &[]).unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert!(stream.close_called);
}
