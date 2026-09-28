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

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    BatchRequest, CopError, DagRequest, Datum, EncodeType, ExecutorSpec, KeyRange, MemoryReader,
    Request, RequestPayload, coprHandler,
};

fn dag_request(executors: Vec<ExecutorSpec>) -> Request {
    Request {
        ranges: vec![KeyRange {
            start: b"k000".to_vec(),
            end: b"k999".to_vec(),
        }],
        start_ts: 1,
        payload: RequestPayload::Dag(DagRequest {
            executors,
            output_offsets: vec![0],
            encode_type: EncodeType::Default,
            collect_execution_summaries: false,
        }),
    }
}

#[test]
fn batch_cop_drains_each_request_into_one_chunk_like_go() {
    let rows = (0..65)
        .map(|index| {
            (
                format!("k{index:03}").into_bytes(),
                (vec![Datum::Int(index)], 1),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let handler = coprHandler::new(Arc::new(MemoryReader { rows }));

    let mut client = handler
        .handleBatchCopRequest(&BatchRequest {
            requests: vec![dag_request(vec![ExecutorSpec::TableScan {
                descending: false,
            }])],
        })
        .unwrap();
    let response = client.Recv().unwrap();

    assert_eq!(response.responses.len(), 1);
    assert_eq!(response.responses[0].chunks.len(), 1);
    assert_eq!(response.responses[0].chunks[0].rows.len(), 65);
}

#[test]
fn batch_cop_returns_dag_build_errors_instead_of_streaming_them() {
    let handler = coprHandler::new(Arc::new(MemoryReader::default()));

    let error = match handler.handleBatchCopRequest(&BatchRequest {
        requests: vec![dag_request(Vec::new())],
    }) {
        Ok(_) => panic!("invalid DAG must fail before a batch client is returned"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        CopError::InvalidRequest("DAG request has no executors".into())
    );
}
