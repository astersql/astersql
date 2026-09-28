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

use crate::deadlock::{DeadlockRequest, DetectorServer, RequestType};
use crate::detector::WaitForEntry;

fn detect_request(
    txn: u64,
    wait_for_txn: u64,
    key_hash: u64,
    key: &[u8],
    resource_group_tag: &[u8],
) -> DeadlockRequest {
    DeadlockRequest {
        request_type: RequestType::Detect,
        entry: WaitForEntry {
            txn,
            wait_for_txn,
            key_hash,
            key: key.to_vec(),
            resource_group_tag: resource_group_tag.to_vec(),
        },
    }
}

#[test]
fn deadlock_response_entry_matches_go_proto_conversion() {
    let server = DetectorServer::new();
    assert!(
        server
            .detect(&detect_request(1, 2, 11, b"first-key", b"first-tag"))
            .is_none()
    );

    let response = server
        .detect(&detect_request(2, 1, 22, b"trigger-key", b"trigger-tag"))
        .expect("the second edge must close the cycle");

    assert_eq!(
        (2, 1, 22),
        (
            response.entry.txn,
            response.entry.wait_for_txn,
            response.entry.key_hash
        )
    );
    assert!(response.entry.key.is_empty());
    assert!(response.entry.resource_group_tag.is_empty());
    assert_eq!(11, response.deadlock_key_hash);
    assert_eq!(2, response.wait_chain.len());
    assert_eq!(b"first-key", response.wait_chain[0].key.as_slice());
    assert_eq!(
        b"trigger-tag",
        response.wait_chain[1].resource_group_tag.as_slice()
    );
}
