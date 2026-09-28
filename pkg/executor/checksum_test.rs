// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `ADMIN CHECKSUM TABLE` 执行器相关单元测试。
//
// 校验：
// - 分区表按物理表（含分区定义 + 逻辑表）×（表扫描 + Public 索引）计请求数；
// - 多路 checksum 响应按 CRC64-XOR 合并，并累加 KV/字节总量。

use crate::checksum::{
    ChecksumAlgorithm, ChecksumBackend, ChecksumOutputChunk, ChecksumRequestSpec, ChecksumResponse,
    ChecksumResultStream, ChecksumScanOn, ChecksumTableExec, DatabaseInfo, IndexInfo,
    PartitionDefinition, PartitionInfo, SchemaState, TableInfo, checksumRequestCount,
    updateChecksumResponse,
};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::num::ParseIntError;
use std::sync::{Arc, Mutex};

/// 分区表：逻辑表 + 2 个分区 = 3 个物理表；每个物理表 1 次表扫描 + 1 个 Public 索引 = 6 请求。
#[test]
fn partitioned_table_checksum_builds_table_and_index_requests_per_physical_table() {
    let table = TableInfo {
        id: 10,
        name: "t".to_owned(),
        is_common_handle: false,
        indices: vec![
            IndexInfo {
                id: 1,
                state: SchemaState::Public,
            },
            // 非 Public 索引不参与 checksum 请求计数。
            IndexInfo {
                id: 2,
                state: SchemaState::Other,
            },
        ],
        partition: Some(PartitionInfo {
            definitions: vec![
                PartitionDefinition { id: 11 },
                PartitionDefinition { id: 12 },
            ],
        }),
    };
    assert_eq!(checksumRequestCount(&table), 6);
}

/// 响应合并：checksum 按位异或，total_kvs / total_bytes 累加。
#[test]
fn checksum_responses_xor_crc_and_sum_kv_totals() {
    let mut response = ChecksumResponse {
        checksum: 0x55,
        total_kvs: 2,
        total_bytes: 8,
    };
    updateChecksumResponse(
        &mut response,
        &ChecksumResponse {
            checksum: 0xaa,
            total_kvs: 4,
            total_bytes: 16,
        },
    );
    assert_eq!(
        response,
        ChecksumResponse {
            // 0x55 ^ 0xaa = 0xff
            checksum: 0xff,
            total_kvs: 6,
            total_bytes: 24,
        }
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestError(String);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestRange {
    NotNull,
    SignedInt,
    Full,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecordedRequest {
    scan_on: ChecksumScanOn,
    algorithm: ChecksumAlgorithm,
    physical_table_id: i64,
    index_id: Option<i64>,
    common_handle: bool,
    ranges: Vec<TestRange>,
    start_ts: u64,
    concurrency: usize,
    resource_group_name: String,
    request_source: String,
}

#[derive(Default)]
struct BackendState {
    requests: Vec<RecordedRequest>,
    open_count: usize,
    kill_count: usize,
    after_count: usize,
    warnings: Vec<String>,
}

struct TestBackend {
    state: Arc<Mutex<BackendState>>,
    checksum_concurrency: String,
    close_error: bool,
}

struct TestStream {
    chunks: VecDeque<Vec<u8>>,
    close_error: bool,
}

impl ChecksumResultStream for TestStream {
    type Context = ();
    type Error = TestError;

    fn next_raw(&mut self, _context: &Self::Context) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.chunks.pop_front())
    }

    fn close(&mut self) -> Result<(), Self::Error> {
        if self.close_error {
            Err(TestError("close failed".to_owned()))
        } else {
            Ok(())
        }
    }
}

impl ChecksumBackend for TestBackend {
    type Context = ();
    type Error = TestError;
    type Request = usize;
    type Range = TestRange;
    type ResourceGroupTagger = &'static str;
    type ResourceGroupName = String;
    type RequestSourceType = String;
    type ResultStream = TestStream;

    fn open_base(&self, _context: &Self::Context) -> Result<(), Self::Error> {
        self.state.lock().unwrap().open_count += 1;
        Ok(())
    }

    fn session_context(&self) -> Self::Context {}

    fn checksum_concurrency_variable(&self) -> Result<String, Self::Error> {
        Ok(self.checksum_concurrency.clone())
    }

    fn invalid_checksum_concurrency(&self, value: &str, error: ParseIntError) -> Self::Error {
        TestError(format!("invalid concurrency {value}: {error}"))
    }

    fn zero_checksum_concurrency(&self) -> Self::Error {
        TestError("zero concurrency".to_owned())
    }

    fn full_not_null_range(&self) -> Vec<Self::Range> {
        vec![TestRange::NotNull]
    }

    fn full_int_range(&self, unsigned: bool) -> Vec<Self::Range> {
        assert!(!unsigned);
        vec![TestRange::SignedInt]
    }

    fn full_range(&self) -> Vec<Self::Range> {
        vec![TestRange::Full]
    }

    fn dist_sql_scan_concurrency(&self) -> usize {
        15
    }

    fn resource_group_tagger(&self) -> Self::ResourceGroupTagger {
        "tagger"
    }

    fn resource_group_name(&self) -> Self::ResourceGroupName {
        "rg".to_owned()
    }

    fn explicit_request_source_type(&self) -> Self::RequestSourceType {
        "admin".to_owned()
    }

    fn build_request(
        &self,
        request: ChecksumRequestSpec<
            Self::Range,
            Self::ResourceGroupTagger,
            Self::ResourceGroupName,
            Self::RequestSourceType,
        >,
    ) -> Result<Self::Request, Self::Error> {
        assert_eq!(request.resource_group_tagger, "tagger");
        let mut state = self.state.lock().unwrap();
        state.requests.push(RecordedRequest {
            scan_on: request.scan_on,
            algorithm: request.algorithm,
            physical_table_id: request.physical_table_id,
            index_id: request.index_id,
            common_handle: request.common_handle,
            ranges: request.ranges,
            start_ts: request.start_ts,
            concurrency: request.concurrency,
            resource_group_name: request.resource_group_name,
            request_source: request.explicit_request_source_type,
        });
        Ok(state.requests.len() - 1)
    }

    fn handle_kill_signal(&self) -> Result<(), Self::Error> {
        self.state.lock().unwrap().kill_count += 1;
        Ok(())
    }

    fn checksum(&self, _request: &Self::Request) -> Result<Self::ResultStream, Self::Error> {
        Ok(TestStream {
            chunks: VecDeque::from([vec![1]]),
            close_error: self.close_error,
        })
    }

    fn decode_checksum_response(&self, data: &[u8]) -> Result<ChecksumResponse, Self::Error> {
        let value = u64::from(data[0]);
        Ok(ChecksumResponse {
            checksum: value,
            total_kvs: value,
            total_bytes: value,
        })
    }

    fn after_handle_checksum_request(&self) {
        self.state.lock().unwrap().after_count += 1;
    }

    fn warn_checksum_failed(&self, _context: &Self::Context, error: &Self::Error) {
        self.state.lock().unwrap().warnings.push(error.0.clone());
    }

    fn info_checksum_result(
        &self,
        _context: &Self::Context,
        _table_id: i64,
        _physical_table_id: i64,
        _index_id: i64,
        _response: &ChecksumResponse,
    ) {
    }
}

#[derive(Default)]
struct TestChunk {
    rows: Vec<Vec<String>>,
}

impl ChecksumOutputChunk for TestChunk {
    fn reset(&mut self) {
        self.rows.clear();
    }

    fn append_string(&mut self, column: usize, value: &str) {
        if self.rows.is_empty() {
            self.rows.push(vec![String::new(); 5]);
        }
        self.rows[0][column] = value.to_owned();
    }

    fn append_u64(&mut self, column: usize, value: u64) {
        self.rows[0][column] = value.to_string();
    }
}

fn partitioned_executor(
    close_error: bool,
) -> (ChecksumTableExec<TestBackend>, Arc<Mutex<BackendState>>) {
    let state = Arc::new(Mutex::new(BackendState::default()));
    let table = TableInfo {
        id: 10,
        name: "t".to_owned(),
        is_common_handle: false,
        indices: vec![IndexInfo {
            id: 20,
            state: SchemaState::Public,
        }],
        partition: Some(PartitionInfo {
            definitions: vec![
                PartitionDefinition { id: 11 },
                PartitionDefinition { id: 12 },
            ],
        }),
    };
    let context = crate::checksum::newChecksumContext(
        DatabaseInfo {
            name: "test".to_owned(),
        },
        table,
        99,
    );
    (
        ChecksumTableExec {
            BaseExecutor: TestBackend {
                state: Arc::clone(&state),
                checksum_concurrency: "2".to_owned(),
                close_error,
            },
            tables: HashMap::from([(10, context)]),
            done: false,
        },
        state,
    )
}

/// 对应 Go TestChecksum：3 个物理表各发出表与索引请求，6 个单值响应汇总为 `0 6 6`。
#[test]
fn checksum_executor_matches_go_partitioned_table_result_and_request_contract() {
    let (mut executor, state) = partitioned_executor(false);
    executor.Open(()).unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.open_count, 1);
    assert_eq!(state.requests.len(), 6);
    assert_eq!(state.kill_count, 12);
    assert_eq!(state.after_count, 6);
    for physical_table_id in [10, 11, 12] {
        assert!(state.requests.contains(&RecordedRequest {
            scan_on: ChecksumScanOn::Table,
            algorithm: ChecksumAlgorithm::Crc64Xor,
            physical_table_id,
            index_id: None,
            common_handle: false,
            ranges: vec![TestRange::SignedInt],
            start_ts: 99,
            concurrency: 15,
            resource_group_name: "rg".to_owned(),
            request_source: "admin".to_owned(),
        }));
        assert!(state.requests.contains(&RecordedRequest {
            scan_on: ChecksumScanOn::Index,
            algorithm: ChecksumAlgorithm::Crc64Xor,
            physical_table_id,
            index_id: Some(20),
            common_handle: false,
            ranges: vec![TestRange::Full],
            start_ts: 99,
            concurrency: 15,
            resource_group_name: "rg".to_owned(),
            request_source: "admin".to_owned(),
        }));
    }
    drop(state);

    let mut output = TestChunk::default();
    executor.Next((), &mut output).unwrap();
    assert_eq!(output.rows, vec![vec!["test", "t", "0", "6", "6"]]);
    executor.Next((), &mut output).unwrap();
    assert!(output.rows.is_empty());
}

/// Go 的 defer 约定：结果流 Close 失败覆盖此前成功结果，并且 failpoint 钩子仍逐请求执行。
#[test]
fn checksum_close_error_is_reported_after_every_stream_is_finalized() {
    let (mut executor, state) = partitioned_executor(true);
    assert_eq!(
        executor.Open(()).unwrap_err(),
        TestError("close failed".to_owned())
    );
    let state = state.lock().unwrap();
    assert_eq!(state.after_count, 6);
    assert_eq!(state.warnings, vec!["close failed"; 6]);
}
