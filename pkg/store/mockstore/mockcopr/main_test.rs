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

// mockcopr 行为测试。
//
// 对应 Go 侧 `TestMain`：先做公共 setup，再登记后台 goroutine 顶函数，
// 避免 leveldb / glog 等长驻线程被误报为泄漏。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    DagRequest, Datum, EncodeType, ExecutorSpec, Expr, KeyRange, KvReader, MemoryReader, Request,
    RequestPayload, Row, coprHandler,
};

#[test]
fn expr_uses_sql_null_semantics() {
    let null = Expr::Constant(Datum::Null);
    assert_eq!(
        Expr::Eq(Box::new(null.clone()), Box::new(null.clone()))
            .eval(&[])
            .unwrap(),
        Datum::Null
    );
    assert_eq!(Expr::Not(Box::new(null)).eval(&[]).unwrap(), Datum::Null);
}

#[test]
fn memory_reader_supports_open_ended_and_descending_ranges() {
    let mut rows = BTreeMap::new();
    rows.insert(b"b".to_vec(), (Row::from([Datum::Int(2)]), 1));
    rows.insert(b"c".to_vec(), (Row::from([Datum::Int(3)]), 1));
    rows.insert(b"d".to_vec(), (Row::from([Datum::Int(4)]), 1));
    let reader = MemoryReader { rows };
    let ranges = [KeyRange {
        start: b"c".to_vec(),
        end: Vec::new(),
    }];

    let result = reader.scan(&ranges, 1, true).unwrap();
    assert_eq!(
        result
            .iter()
            .map(|pair| pair.key.as_slice())
            .collect::<Vec<_>>(),
        vec![b"d".as_slice(), b"c".as_slice()]
    );
}

#[test]
fn dag_execution_summaries_follow_request_flag() {
    let mut rows = BTreeMap::new();
    rows.insert(b"a".to_vec(), (Row::from([Datum::Int(1)]), 1));
    let handler = coprHandler::new(Arc::new(MemoryReader { rows }));
    let request = Request {
        ranges: vec![KeyRange {
            start: b"a".to_vec(),
            end: b"b".to_vec(),
        }],
        start_ts: 1,
        payload: RequestPayload::Dag(DagRequest {
            executors: vec![ExecutorSpec::TableScan { descending: false }],
            output_offsets: vec![0],
            encode_type: EncodeType::Default,
            collect_execution_summaries: false,
        }),
    };

    let response = handler.handleCopDAGRequest(&request);
    assert!(response.execution_summaries.is_empty());
}
