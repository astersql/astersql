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

// 单目标 DataSink 的 AsterSQL 迁移补充单元测试。
//
// 覆盖有界队列非阻塞语义、SQL/Plan 转发且丢弃 TopRU、地址切换时的注册/注销，
// 以及对 Unimplemented TopRU 流的兼容关闭行为。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::*;
use serial_test::serial;
use tonic::Code;
use topsql_protocol::server::StartMockAgentServer;

/// 记录 Register/Deregister 调用次数的测试用注册器。
#[derive(Default)]
struct RecordingRegisterer {
    registrations: Mutex<usize>,
    deregistrations: Mutex<usize>,
}

impl RecordingRegisterer {
    /// 已注册次数。
    fn registrations(&self) -> usize {
        *self.registrations.lock().unwrap()
    }

    /// 已注销次数。
    fn deregistrations(&self) -> usize {
        *self.deregistrations.lock().unwrap()
    }
}

impl DataSinkRegisterer for RecordingRegisterer {
    fn register(&self, _data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        *self.registrations.lock().unwrap() += 1;
        Ok(())
    }

    fn deregister(&self, _data_sink: &Arc<dyn DataSink>) {
        *self.deregistrations.lock().unwrap() += 1;
    }
}

/// 构造 protobuf-codec TopSQL 记录。
fn protobuf_top_sql(sql: &[u8], plan: &[u8]) -> tipb_protobuf::TopSqlRecord {
    let mut record = tipb_protobuf::TopSqlRecord::new();
    record.set_sql_digest(sql.to_vec());
    record.set_plan_digest(plan.to_vec());
    record
}

/// 构造 protobuf-codec TopRU 记录。
fn protobuf_top_ru(sql: &[u8], ru: f64) -> tipb_protobuf::TopRuRecord {
    let mut record = tipb_protobuf::TopRuRecord::new();
    record.set_sql_digest(sql.to_vec());
    let _ = ru;
    record
}

/// 构造 protobuf-codec SQL 元数据。
fn protobuf_sql_meta(sql: &[u8], normalized: &str) -> tipb_protobuf::SqlMeta {
    let mut meta = tipb_protobuf::SqlMeta::new();
    meta.set_sql_digest(sql.to_vec());
    meta.set_normalized_sql(normalized.to_owned());
    meta
}

/// 构造 protobuf-codec Plan 元数据。
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

#[test]
#[serial]
/// 验证容量为 1 时第二次 TrySend 得 ChannelFull，Close 后得 Closed。
fn bounded_queue_and_close_match_go_non_blocking_semantics() {
    let registerer = Arc::new(RecordingRegisterer::default());
    let address = Arc::new(MutableReceiverAddress::default());
    let sink = NewSingleTargetDataSinkWithReceiver(registerer, address);

    sink.TrySend(
        ReportData::default(),
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    let error = sink
        .TrySend(
            ReportData::default(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap_err();
    assert_eq!(error, SingleTargetError::ChannelFull);

    sink.Close();
    let error = sink
        .TrySend(
            ReportData::default(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap_err();
    assert_eq!(error, SingleTargetError::Closed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
/// 验证转发 TopSQL/元数据、丢弃 TopRU，以及地址清空/切换时的注销与再注册。
async fn forwards_sql_payloads_drops_top_ru_and_switches_registration() {
    let mut first = StartMockAgentServer().await.expect("start first server");
    let mut second = StartMockAgentServer().await.expect("start second server");
    let registerer = Arc::new(RecordingRegisterer::default());
    let address = Arc::new(MutableReceiverAddress::new(first.Address()));
    let sink = NewSingleTargetDataSinkWithReceiver(registerer.clone(), address.clone());
    sink.SetPollInterval(Duration::from_millis(10));
    sink.Start();
    assert_eq!(registerer.registrations(), 1);

    sink.TrySend(
        ReportData {
            data_records: vec![protobuf_top_sql(b"S1", b"P1")],
            ru_records: vec![protobuf_top_ru(b"S1", 1.5)],
            sql_metas: vec![protobuf_sql_meta(b"S1", "SQL-1")],
            plan_metas: vec![protobuf_plan_meta(b"P1", "PLAN-1")],
        },
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();

    first.WaitCollectCnt(0, 1, Duration::from_secs(2));
    first.WaitCollectCntOfSQLMeta(0, 1, Duration::from_secs(2));
    wait_until(Duration::from_secs(2), || {
        first.GetPlanMetaByDigestBlocking(b"P1", Duration::ZERO).1
    });
    assert_eq!(first.RURecordsCnt(), 0);
    assert_eq!(first.GetLatestRecords().unwrap().len(), 1);
    assert_eq!(first.GetTotalSQLMetas()[0].normalized_sql, "SQL-1");
    assert_eq!(
        first.GetPlanMetaByDigestBlocking(b"P1", Duration::ZERO).0,
        "PLAN-1"
    );

    address.Set(String::new());
    wait_until(Duration::from_secs(1), || registerer.deregistrations() == 1);
    address.Set(second.Address());
    wait_until(Duration::from_secs(1), || registerer.registrations() == 2);

    sink.TrySend(
        ReportData {
            data_records: vec![protobuf_top_sql(b"S2", b"P2")],
            ..ReportData::default()
        },
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    second.WaitCollectCnt(0, 1, Duration::from_secs(2));
    assert_eq!(second.GetLatestRecords().unwrap()[0].sql_digest, b"S2");

    sink.Close();
    wait_until(Duration::from_secs(1), || registerer.deregistrations() == 2);
    first.Stop();
    second.Stop();
}

/// 可注入 Send/Close 错误的 TopRU 流桩。
struct MockTopRUStream {
    send_error: Option<tonic::Status>,
    close_error: Option<tonic::Status>,
    close_called: bool,
}

impl TopRURecordStream for MockTopRUStream {
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

#[test]
/// 验证 Send 返回 Unimplemented 时关闭流并视为兼容；其它 Close 错误仍上抛。
fn unimplemented_top_ru_stream_is_closed_and_treated_as_compatible() {
    let mut stream = MockTopRUStream {
        send_error: Some(tonic::Status::unimplemented("topru rpc not supported")),
        close_error: Some(tonic::Status::internal("close failed")),
        close_called: false,
    };
    let sent = sendTopRURecords(
        &mut stream,
        &[tipb::TopRuRecord {
            sql_digest: b"S1".to_vec(),
            ru: 1.5,
        }],
    )
    .unwrap();
    assert_eq!(sent, 0);
    assert!(stream.close_called);

    let mut stream = MockTopRUStream {
        send_error: None,
        close_error: Some(tonic::Status::internal("close failed")),
        close_called: false,
    };
    let error = sendTopRURecords(&mut stream, &[]).unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert!(stream.close_called);
}
