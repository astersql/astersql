// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

use std::sync::Arc;

use crate::{
    CopError, DagRequest, ExecutorSpec, MemoryReader, Request, RequestPayload, coprHandler,
    mockBathCopErrClient,
};

#[test]
fn dag_request_rejects_empty_ranges_like_go() {
    let handler = coprHandler::new(Arc::new(MemoryReader::default()));
    let request = Request {
        ranges: Vec::new(),
        start_ts: 1,
        payload: RequestPayload::Dag(DagRequest {
            executors: vec![ExecutorSpec::TableScan { descending: false }],
            ..DagRequest::default()
        }),
    };

    let error = match handler.buildDAGExecutor(&request) {
        Ok(_) => panic!("empty ranges must be rejected"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        CopError::InvalidRequest("request range is null".into())
    );
}

#[test]
fn batch_error_client_repeats_go_error_response() {
    let mut client = mockBathCopErrClient {
        Error: CopError::Region("region unavailable".into()),
    };

    for _ in 0..2 {
        let response = client.Recv().expect("Go returns the error response");
        assert!(response.responses.is_empty());
        assert_eq!(response.other_error.as_deref(), Some("region unavailable"));
    }
}
