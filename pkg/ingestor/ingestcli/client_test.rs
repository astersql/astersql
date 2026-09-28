// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// ingestcli client 单元测试：用内存 MockTransport 覆盖写流与 ingest。
//
// 对齐 Go httptest.Server 行为：断言请求 URL/字节、返回 canned HttpResponse；
// 指标断言用 `>=` 因 cargo test 并发共享直方图。SST：有序键值文件；ingest：导入 Region。

// Ported from pkg/ingestor/ingestcli/client_test.go. Go's `httptest.Server`
// backs each case with a real HTTP listener; this crate injects the
// `HttpTransport` trait instead, so these tests use an in-process mock
// transport that asserts on the exact request bytes/URL and returns a canned
// `HttpResponse`, which is the Rust-native equivalent of the same behavior.
// The metric assertions use `>=` instead of Go's exact `before+1` equality:
// `cargo test` runs test functions concurrently on one process (unlike Go's
// default sequential `t.Run`), so other tests in this binary may also
// observe into the same shared histogram; the `>=` bound still proves this
// test's own call was recorded.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_ingestor_ingestmetric as ingestmetric;

use crate::{
    Client, ClientImpl, Error, HttpResponse, HttpTransport, IngestRequest, Pair, Peer, Region,
    RegionEpoch, RegionInfo, SplitClient, Store, WriteRequest, WriteResponse,
};

/// put_stream 回调：收到完整 body 后返回 HttpResponse。
type PutStreamHandler = Box<dyn Fn(&str, Vec<u8>) -> Result<HttpResponse, Error> + Send + Sync>;
/// post 回调。
type PostHandler = Box<dyn Fn(&str, &[u8]) -> Result<HttpResponse, Error> + Send + Sync>;

/// 进程内注入的 HttpTransport，将真实 HTTP 替换为可断言的闭包。
struct MockTransport {
    put_stream: PutStreamHandler,
    post: PostHandler,
}

impl HttpTransport for MockTransport {
    fn put_stream(
        &self,
        url: &str,
        chunks: mpsc::Receiver<Vec<u8>>,
    ) -> Result<HttpResponse, Error> {
        // 聚合 chunked 写入体，交给用例断言。
        let body: Vec<u8> = chunks.into_iter().flatten().collect();
        (self.put_stream)(url, body)
    }

    fn post(&self, url: &str, body: &[u8]) -> Result<HttpResponse, Error> {
        (self.post)(url, body)
    }
}

/// 本用例不应调用 put_stream。
fn unreachable_put_stream() -> PutStreamHandler {
    Box::new(|_url, _body| unreachable!("put_stream should not be called in this test"))
}

/// 本用例不应调用 post。
fn unreachable_post() -> PostHandler {
    Box::new(|_url, _body| unreachable!("post should not be called in this test"))
}

/// 固定 status_address 的 SplitClient 桩。
struct StubSplitClient {
    status_address: String,
}

impl SplitClient for StubSplitClient {
    fn get_store(
        &self,
        _context: &dyn crate::RequestContext,
        _store_id: u64,
    ) -> Result<Store, Error> {
        Ok(Store {
            status_address: self.status_address.clone(),
        })
    }
}

fn stub_split_client(status_address: impl Into<String>) -> Arc<StubSplitClient> {
    Arc::new(StubSplitClient {
        status_address: status_address.into(),
    })
}

/// 构造写流成功响应 JSON（含 sst_meta）。
fn sst_meta_response_body(id: i64, meta_offset: usize, commit_ts: usize) -> String {
    format!(
        "{{\"sst_meta\":{{\"id\":{id},\"smallest\":[0],\"biggest\":[1],\"meta-offset\":{meta_offset},\"commit-ts\":{commit_ts}}}}}"
    )
}

/// 手工编码仅含 message 字段的 errorpb.Error（field 1, wire 2）。
fn encode_error_pb_message(message: &str) -> Vec<u8> {
    let mut bytes = vec![0x0a_u8];
    encode_varint(message.len() as u64, &mut bytes);
    bytes.extend_from_slice(message.as_bytes());
    bytes
}

/// 编码 protobuf varint。
fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

/// 构造单对 KV 的 WriteRequest。
fn write_request(key: &[u8], value: &[u8]) -> WriteRequest {
    WriteRequest {
        pairs: vec![Pair {
            key: key.to_vec(),
            value: value.to_vec(),
        }],
    }
}

// The store address that `ingest()` will POST to comes from the split
// client (see `stub_split_client`), not the request; the request only
// carries the region/leader used to look up that store.
/// 构造带固定 Region/leader 与给定 SST id 的 IngestRequest。
fn ingest_request(sst_id: i64) -> IngestRequest {
    IngestRequest {
        region: RegionInfo {
            region: Region {
                id: 1,
                region_epoch: Some(RegionEpoch {
                    conf_ver: 0,
                    version: 1,
                }),
                ..Default::default()
            },
            leader: Some(Peer { id: 1, store_id: 1 }),
        },
        write_response: WriteResponse {
            next_gen_sst_meta: Some(crate::NextGenSstMeta {
                id: sst_id,
                ..Default::default()
            }),
        },
    }
}

/// 校验 JsonByteSlice 编码为 JSON 整数数组。
#[test]
fn test_json_byte_slice() {
    let slice = crate::JsonByteSlice(Some(vec![3, 2, 0, 2, 255]));
    assert_eq!("[3,2,0,2,255]", crate::encode_json_bytes(&slice));
}

/// Go's encoding/json keeps zero values for omitted response fields and ignores
/// unknown fields, including values outside the subset consumed by this client.
#[test]
fn test_write_response_json_matches_go_defaults_and_unknown_fields() {
    let meta =
        crate::decode_next_gen_response(br#"{"sst_meta":{"id":7},"future":true,"ratio":1.25}"#)
            .expect("Go accepts omitted fields and unknown JSON values");

    assert_eq!(7, meta.id);
    assert_eq!(None, meta.smallest.0);
    assert_eq!(None, meta.biggest.0);
    assert_eq!(0, meta.meta_offset);
    assert_eq!(0, meta.commit_ts);
}

/// A missing sst_meta object decodes to the zero-valued embedded struct in Go.
#[test]
fn test_write_response_json_missing_sst_meta_uses_zero_value() {
    assert_eq!(
        crate::NextGenSstMeta::default(),
        crate::decode_next_gen_response(br#"{}"#)
            .expect("Go leaves a missing sst_meta at its zero value")
    );
}

/// 写流应发出正确 chunk 字节，并解析 sst_meta。
#[test]
fn test_write_client_write_chunk() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let response_body = sst_meta_response_body(1, 1, 1);
    let transport = Arc::new(MockTransport {
        put_stream: Box::new(move |url, body| {
            assert!(url.contains("/write_sst?cluster_id=12345&commit_ts=67890"));
            *captured_clone.lock().unwrap() = body;
            Ok(HttpResponse {
                status_code: 200,
                body: response_body.clone().into_bytes(),
            })
        }),
        post: unreachable_post(),
    });

    let client = ClientImpl::new(
        "http://tikv-worker",
        12345,
        false,
        transport,
        stub_split_client(""),
    );
    let mut write_client = client.write_client(&(), 67890).expect("write_client");
    write_client
        .write(write_request(b"key", b"value"))
        .expect("write");
    let response = write_client.recv().expect("recv");

    // 线格式：u16 LE key_len + key + u32 LE value_len + value。
    assert_eq!(
        b"\x03\x00key\x05\x00\x00\x00value".to_vec(),
        *captured.lock().unwrap()
    );
    let sst_meta = response.sst_meta().expect("sst meta");
    assert_eq!(1, sst_meta.id);
    assert_eq!(Some(vec![0]), sst_meta.smallest.0);
    assert_eq!(Some(vec![1]), sst_meta.biggest.0);
    assert_eq!(1, sst_meta.meta_offset);
    assert_eq!(1, sst_meta.commit_ts);
}

/// 服务端 500 时 write 仍可缓冲，错误在 recv 时暴露。
#[test]
fn test_client_write_server_error() {
    let transport = Arc::new(MockTransport {
        put_stream: Box::new(|_url, _body| {
            Ok(HttpResponse {
                status_code: 500,
                body: b"internal server error".to_vec(),
            })
        }),
        post: unreachable_post(),
    });

    let client = ClientImpl::new(
        "http://tikv-worker",
        12345,
        false,
        transport,
        stub_split_client(""),
    );
    let mut write_client = client.write_client(&(), 67890).expect("write_client");
    // The write itself succeeds; the error only surfaces once the worker
    // thread's response is collected in `recv`, matching Go's comment
    // "Error only return when pipeWriter is closed?".
    write_client
        .write(write_request(b"key", b"value"))
        .expect("write should buffer successfully");
    let error = write_client
        .recv()
        .expect_err("recv should surface the HTTP failure");
    assert!(error.to_string().contains("internal server error"));
}

/// WriteAPIDuration 直方图应对一次写流至少 observe 一次，且耗时累加合理。
#[test]
fn test_write_client_duration_metric_observe_once() {
    ingestmetric::InitIngestMetrics();
    let before = histogram_sample_count(&ingestmetric::WriteAPIDuration);
    let before_sum = histogram_sample_sum(&ingestmetric::WriteAPIDuration);

    let response_body = sst_meta_response_body(1, 1, 1);
    let transport = Arc::new(MockTransport {
        put_stream: Box::new(move |_url, _body| {
            thread::sleep(Duration::from_millis(80));
            Ok(HttpResponse {
                status_code: 200,
                body: response_body.clone().into_bytes(),
            })
        }),
        post: unreachable_post(),
    });

    let client = ClientImpl::new(
        "http://tikv-worker",
        12345,
        false,
        transport,
        stub_split_client(""),
    );
    let mut write_client = client.write_client(&(), 67890).expect("write_client");
    write_client
        .write(write_request(b"key1", b"value1"))
        .expect("write key1");
    write_client
        .write(write_request(b"key2", b"value2"))
        .expect("write key2");
    write_client.recv().expect("recv");

    let after = histogram_sample_count(&ingestmetric::WriteAPIDuration);
    let after_sum = histogram_sample_sum(&ingestmetric::WriteAPIDuration);
    assert!(after >= before + 1);
    assert!(after_sum - before_sum >= 0.05);
}

/// 成功 ingest 应 POST 到 /ingest_s3。
#[test]
fn test_client_ingest() {
    let transport = Arc::new(MockTransport {
        put_stream: unreachable_put_stream(),
        post: Box::new(|url, _body| {
            assert!(url.contains("/ingest_s3"));
            Ok(HttpResponse {
                status_code: 200,
                body: Vec::new(),
            })
        }),
    });

    let client = ClientImpl::new(
        "http://tikv-worker",
        12345,
        false,
        transport,
        stub_split_client("store-addr"),
    );
    let request = ingest_request(1);
    client.ingest(&(), request).expect("ingest should succeed");
}

/// IngestAPIDuration 直方图应对成功 ingest 至少 observe 一次。
#[test]
fn test_client_ingest_duration_metric() {
    ingestmetric::InitIngestMetrics();
    let before = histogram_sample_count(&ingestmetric::IngestAPIDuration);
    let before_sum = histogram_sample_sum(&ingestmetric::IngestAPIDuration);

    let transport = Arc::new(MockTransport {
        put_stream: unreachable_put_stream(),
        post: Box::new(|url, _body| {
            assert!(url.contains("/ingest_s3"));
            thread::sleep(Duration::from_millis(80));
            Ok(HttpResponse {
                status_code: 200,
                body: Vec::new(),
            })
        }),
    });

    let client = ClientImpl::new(
        "http://tikv-worker",
        12345,
        false,
        transport,
        stub_split_client("store-addr"),
    );
    let request = ingest_request(1);
    client.ingest(&(), request).expect("ingest should succeed");

    let after = histogram_sample_count(&ingestmetric::IngestAPIDuration);
    let after_sum = histogram_sample_sum(&ingestmetric::IngestAPIDuration);
    assert!(after >= before + 1);
    assert!(after_sum - before_sum >= 0.05);
}

/// ingest 非 200 时应解码 errorpb 并附带 SST ID。
#[test]
fn test_client_ingest_error() {
    let transport = Arc::new(MockTransport {
        put_stream: unreachable_put_stream(),
        post: Box::new(|_url, _body| {
            Ok(HttpResponse {
                status_code: 500,
                body: encode_error_pb_message("test error"),
            })
        }),
    });

    let client = ClientImpl::new(
        "http://tikv-worker",
        12345,
        false,
        transport,
        stub_split_client("store-addr"),
    );
    let request = ingest_request(123456);
    let error = client
        .ingest(&(), request)
        .expect_err("ingest should surface the errorpb response");
    let message = error.to_string();
    assert!(message.contains("test error"));
    assert!(message.contains("ingest SST ID 123456"));
}

/// 校验 NewClient URL 补全：裸 host 加 schema；已有 schema 则保留。
#[test]
fn test_next_client_url() {
    struct Case {
        in_url: &'static str,
        for_http: &'static str,
        for_https: &'static str,
    }
    let cases = [
        Case {
            in_url: "localhost:9000",
            for_http: "http://localhost:9000",
            for_https: "https://localhost:9000",
        },
        Case {
            in_url: "http://localhost:9000",
            for_http: "http://localhost:9000",
            for_https: "http://localhost:9000",
        },
        Case {
            in_url: "https://localhost:9000",
            for_http: "https://localhost:9000",
            for_https: "https://localhost:9000",
        },
    ];

    for case in cases {
        let transport = Arc::new(MockTransport {
            put_stream: unreachable_put_stream(),
            post: unreachable_post(),
        });
        let client = ClientImpl::new(
            case.in_url,
            1,
            false,
            transport.clone(),
            stub_split_client(""),
        );
        assert_eq!("http://", client.url_schema);
        assert_eq!(case.for_http, client.tikv_worker_url);

        let client = ClientImpl::new(case.in_url, 1, true, transport, stub_split_client(""));
        assert_eq!("https://", client.url_schema);
        assert_eq!(case.for_https, client.tikv_worker_url);
    }
}

/// 读取 Prometheus Histogram 样本计数。
fn histogram_sample_count(histogram: &std::sync::RwLock<Option<prometheus::Histogram>>) -> u64 {
    let guard = histogram.read().expect("histogram lock poisoned");
    guard
        .as_ref()
        .expect("InitIngestMetrics must run first")
        .get_sample_count()
}

/// 读取 Prometheus Histogram 样本总和。
fn histogram_sample_sum(histogram: &std::sync::RwLock<Option<prometheus::Histogram>>) -> f64 {
    let guard = histogram.read().expect("histogram lock poisoned");
    guard
        .as_ref()
        .expect("InitIngestMetrics must run first")
        .get_sample_sum()
}
