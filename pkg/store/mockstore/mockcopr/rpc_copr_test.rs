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

use std::sync::Arc;
use std::time::Duration;

use crate::{BatchRequest, CopError, MemoryReader, NewCoprRPCHandlerWithReader, RPCSession};

#[test]
fn batch_cop_region_error_is_returned_in_the_stream_first_response() {
    let reader = Arc::new(MemoryReader::default());
    let mut handler = NewCoprRPCHandlerWithReader(reader.clone());
    let mut session = RPCSession::new(reader);
    session.region_error = Some(CopError::Region("stale epoch".into()));

    let mut stream = handler
        .HandleBatchCop(&session, &BatchRequest::default(), Duration::from_secs(1))
        .expect("Go returns a stream response rather than an RPC error");

    assert_eq!(
        stream
            .BatchResponse
            .as_ref()
            .expect("the region error is pre-fetched as the first response")
            .other_error
            .as_deref(),
        Some("stale epoch")
    );
    assert_eq!(
        stream.Recv().unwrap().other_error.as_deref(),
        Some("stale epoch"),
        "the Go error client repeats the region error on every Recv"
    );
    assert_eq!(stream.Timeout, Duration::ZERO);
}
