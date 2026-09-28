// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// cophandler 单元测试：前缀后继、点查、闭包/MPP 执行与请求冒烟。
//
// 对齐 Go 侧 prefix next、点查空结果、Selection/TopN 以及 tunnel 取消语义。

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use crate::closure_exec::exceed_end_key;
use crate::cop_handler::{
    ByItem, DagRequest, Datum, Executor, Expr, KeyRange, MemoryReader, Request, RequestPayload,
    extract_kv_ranges, handle_cop_request,
};
use crate::mpp::{ExchangerTunnel, MppTaskHandler};
use crate::mpp_exec::{MaterializedExec, MppExec, execute_executor};
use crate::topn::TopNHeap;

/// 就地计算 key 的下一前缀（对齐 Go ConvertToPrefixNext）。
fn convert_to_prefix_next(key: &mut Vec<u8>) -> Vec<u8> {
    if key.is_empty() {
        return vec![0];
    }
    for i in (0..key.len()).rev() {
        if key[i] != 255 {
            key[i] += 1;
            return key.clone();
        }
        key[i] = 0;
    }
    // 全字节进位：全部置 255 后追加 0（与 Go 行为对齐）。
    for byte in key.iter_mut() {
        *byte = 255;
    }
    key.push(0);
    key.clone()
}

/// 校验 `convert_to_prefix_next` 结果是否等于期望。
fn is_prefix_next(mut key: Vec<u8>, expected: Vec<u8>) -> bool {
    convert_to_prefix_next(&mut key) == expected
}

/// 覆盖空键、进位与多字节边界的 prefix next 用例。
#[test]
fn TestIsPrefixNext() {
    assert!(is_prefix_next(vec![], vec![0]));
    assert!(is_prefix_next(vec![0], vec![1]));
    assert!(is_prefix_next(vec![1], vec![2]));
    assert!(is_prefix_next(vec![255], vec![255, 0]));
    assert!(is_prefix_next(vec![255, 255, 255], vec![255, 255, 255, 0]));
    assert!(is_prefix_next(vec![1, 255], vec![2, 0]));
    assert!(is_prefix_next(vec![0, 1, 255], vec![0, 2, 0]));
    assert!(is_prefix_next(vec![0, 1, 255, 5], vec![0, 1, 255, 6]));
    assert!(is_prefix_next(vec![0, 1, 5, 255], vec![0, 1, 6, 0]));
    assert!(is_prefix_next(vec![0, 1, 255, 255], vec![0, 2, 0, 0]));
    assert!(is_prefix_next(vec![0, 255, 255, 255], vec![1, 0, 0, 0]));
}

/// 构造含三行样本数据的 MemoryReader。
fn sample_reader() -> MemoryReader {
    let mut rows = BTreeMap::new();
    rows.insert(
        b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\0".to_vec(),
        (
            vec![Datum::Int(0), Datum::Bytes(b"a".to_vec()), Datum::Real(0.0)],
            1,
        ),
    );
    rows.insert(
        b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\x01".to_vec(),
        (
            vec![Datum::Int(1), Datum::Bytes(b"b".to_vec()), Datum::Real(1.5)],
            2,
        ),
    );
    rows.insert(
        b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\x02".to_vec(),
        (
            vec![Datum::Int(2), Datum::Bytes(b"c".to_vec()), Datum::Real(2.5)],
            3,
        ),
    );
    MemoryReader { rows }
}

/// Go's `extractKVRanges` rejects a non-empty start with an empty request end.
#[test]
fn ExtractKVRangesRejectsOpenEndedRequestRange() {
    let result = extract_kv_ranges(
        &[],
        &[],
        &[KeyRange {
            start: b"b".to_vec(),
            end: Vec::new(),
        }],
        false,
    );

    assert!(matches!(
        result,
        Err(crate::cop_handler::CopError::InvalidRequest(_))
    ));
}

/// Invalid request ranges must be rejected instead of silently producing an empty scan.
#[test]
fn ExtractKVRangesRejectsReversedRange() {
    let result = extract_kv_ranges(
        &[],
        &[],
        &[KeyRange {
            start: b"z".to_vec(),
            end: b"a".to_vec(),
        }],
        false,
    );

    assert!(matches!(
        result,
        Err(crate::cop_handler::CopError::InvalidRequest(_))
    ));
}

/// Checksum response stays equal to the fixed Go unistore mock response.
#[test]
fn ChecksumResponseMatchesGoMock() {
    let response = handle_cop_request(
        &sample_reader(),
        &Request {
            payload: RequestPayload::Checksum,
            ranges: vec![KeyRange {
                start: b"t".to_vec(),
                end: Vec::new(),
            }],
            start_ts: 100,
            resolved_locks: Vec::new(),
            paging_size: 0,
            cache_enabled: false,
            cache_if_match_version: 0,
        },
    );

    assert_eq!(
        response
            .data
            .chunks_exact(8)
            .map(|bytes| u64::from_be_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
}

/// SQL comparisons and NOT preserve NULL instead of turning it into TRUE.
#[test]
fn ExprUsesSqlNullSemantics() {
    let null = Expr::Constant(Datum::Null);
    assert_eq!(
        Expr::Eq(Box::new(null.clone()), Box::new(null.clone()))
            .eval(&[])
            .unwrap(),
        Datum::Null
    );
    assert_eq!(Expr::Not(Box::new(null)).eval(&[]).unwrap(), Datum::Null);
}

/// 点查：无匹配范围返回空；精确范围返回单行。
#[test]
fn TestPointGet() {
    let reader = sample_reader();
    let empty_range = [KeyRange {
        start: b"t\0\0\0\0\0\0\0\0_r\x80\0\0\0\0\0\0\0".to_vec(),
        end: b"t\0\0\0\0\0\0\0\0_r\x80\0\0\0\0\0\0\x01".to_vec(),
    }];
    let output = execute_executor(
        &reader,
        &empty_range,
        100,
        &Executor::TableScan {
            columns: vec![0, 1],
            descending: false,
        },
    )
    .unwrap();
    assert!(output.rows.is_empty());

    let point_range = [KeyRange {
        start: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\0".to_vec(),
        end: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\x01".to_vec(),
    }];
    let output = execute_executor(
        &reader,
        &point_range,
        100,
        &Executor::TableScan {
            columns: vec![0, 1],
            descending: false,
        },
    )
    .unwrap();
    assert_eq!(output.rows.len(), 1);
    assert_eq!(output.rows[0][0], Datum::Int(0));
    assert_eq!(output.rows[0][1], Datum::Bytes(b"a".to_vec()));
}

/// Selection 过滤 + exceed_end_key 辅助断言。
#[test]
fn TestClosureExecutor() {
    let reader = sample_reader();
    let ranges = [KeyRange {
        start: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\0".to_vec(),
        end: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\x03".to_vec(),
    }];
    let output = execute_executor(
        &reader,
        &ranges,
        100,
        &Executor::Selection {
            condition: Expr::Gt(
                Box::new(Expr::Column(0)),
                Box::new(Expr::Constant(Datum::Int(0))),
            ),
            child: Box::new(Executor::TableScan {
                columns: vec![0, 1, 2],
                descending: false,
            }),
        },
    )
    .unwrap();
    assert_eq!(output.rows.len(), 2);
    assert_eq!(output.rows[0][0], Datum::Int(1));
    assert_eq!(output.rows[1][0], Datum::Int(2));
    assert!(!exceed_end_key(&ranges[0].start, &ranges[0].end));
}

/// TopN 降序取前 2 行，并单独验证 TopNHeap 升序行为。
#[test]
fn TestMppExecutor() {
    let reader = sample_reader();
    let ranges = [KeyRange {
        start: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\0".to_vec(),
        end: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\x03".to_vec(),
    }];
    let output = execute_executor(
        &reader,
        &ranges,
        100,
        &Executor::TopN {
            order_by: vec![ByItem {
                expr: Expr::Column(0),
                descending: true,
                enum_unsigned: false,
            }],
            limit: 2,
            child: Box::new(Executor::TableScan {
                columns: vec![0, 1],
                descending: false,
            }),
        },
    )
    .unwrap();
    assert_eq!(output.rows.len(), 2);
    assert_eq!(output.rows[0][0], Datum::Int(2));
    assert_eq!(output.rows[1][0], Datum::Int(1));

    let mut heap = TopNHeap::new(
        2,
        vec![ByItem {
            expr: Expr::Column(0),
            descending: false,
            enum_unsigned: false,
        }],
    );
    for row in [
        vec![Datum::Int(3)],
        vec![Datum::Int(1)],
        vec![Datum::Int(2)],
    ] {
        heap.add_data_row(row).unwrap();
    }
    let sorted = heap.into_sorted_rows().unwrap();
    assert_eq!(sorted[0][0], Datum::Int(1));
    assert_eq!(sorted[1][0], Datum::Int(2));
}

/// tunnel 在连接前关闭应快速返回 Cancelled；MppTaskHandler.cancel 生效。
#[test]
fn TestExchSenderExecNextReturnsWhenCtxCanceledBeforeTunnelConnected() {
    let tunnel = ExchangerTunnel::new();
    let tunnel_for_thread = tunnel.clone();
    let started = Instant::now();
    let handle = thread::spawn(move || tunnel_for_thread.wait_connected());

    thread::sleep(Duration::from_millis(20));
    tunnel.close();
    let result = handle.join().expect("waiter thread");
    assert!(matches!(
        result,
        Err(crate::cop_handler::CopError::Cancelled)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));

    let handler = MppTaskHandler::default();
    handler.cancel();
    assert!(handler.cancelled());
}

/// MaterializedExec 生命周期与 handle_cop_request DAG 冒烟。
#[test]
fn MaterializedExec_and_handle_cop_request_smoke() {
    let mut exec = MaterializedExec::new(vec![vec![Datum::Int(7)]]);
    exec.open().unwrap();
    let batch = exec.next().unwrap().unwrap();
    assert_eq!(batch[0][0], Datum::Int(7));
    assert!(exec.next().unwrap().is_none());
    exec.stop().unwrap();

    let reader = sample_reader();
    let response = handle_cop_request(
        &reader,
        &Request {
            payload: RequestPayload::Dag(DagRequest {
                root: Some(Executor::TableScan {
                    columns: vec![0],
                    descending: false,
                }),
                executors: vec![],
                output_offsets: vec![0],
                collect_range_counts: false,
                encode_chunk: false,
            }),
            ranges: vec![KeyRange {
                start: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\0".to_vec(),
                end: b"t\0\0\0\0\0\0\0\0_r\0\0\0\0\0\0\0\x03".to_vec(),
            }],
            start_ts: 100,
            resolved_locks: Vec::new(),
            paging_size: 0,
            cache_enabled: false,
            cache_if_match_version: 0,
        },
    );
    assert!(response.other_error.is_none());
    assert!(
        !response.chunks.is_empty() || !response.data.is_empty() || !response.summaries.is_empty()
    );
}
