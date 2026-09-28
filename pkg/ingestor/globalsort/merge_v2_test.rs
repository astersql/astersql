// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use crate::merge_v2::MergeOverlappingFilesV2;
use crate::reader::CancellationToken;
use crate::{MemoryStorage, Storage, decode_kvs};

// Go's MergeOverlappingFilesV2 accepts writeBatchCount=0 because that argument
// is not used by the one-file writer path. Keep the Rust port's public behavior
// aligned instead of imposing an additional validation rule.
#[test]
fn merge_v2_accepts_zero_write_batch_count_like_go() {
    let store = MemoryStorage::default();

    let output = MergeOverlappingFilesV2(
        &CancellationToken::default(),
        &[],
        &store,
        b"",
        b"",
        0,
        "/out",
        "zero-batch",
        0,
        0,
        0,
        0,
        None,
        1,
        false,
    )
    .expect("Go parity: zero writeBatchCount is accepted");

    assert_eq!("/out/zero-batch.data", output);
    assert!(
        decode_kvs(&store.read(&output).unwrap(), 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        Vec::<u8>::new(),
        store.read("/out/zero-batch.stat").unwrap()
    );
}
