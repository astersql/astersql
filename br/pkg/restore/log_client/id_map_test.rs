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

use crate::client::TEST_NewLogClient;

#[test]
fn truncated_id_map_metadata_is_rejected() {
    let client = TEST_NewLogClient(42, 1000);

    let truncated_header = b"BM\x2a\x00";
    assert!(client.loadPITRIDMapBackupMeta(truncated_header).is_err());

    let mut truncated_name = Vec::new();
    truncated_name.extend_from_slice(b"BM");
    truncated_name.extend_from_slice(&42_u64.to_le_bytes());
    truncated_name.extend_from_slice(&1_u32.to_le_bytes());
    truncated_name.extend_from_slice(&4_u32.to_le_bytes());
    truncated_name.extend_from_slice(b"abc");
    assert!(client.loadPITRIDMapBackupMeta(&truncated_name).is_err());
}
