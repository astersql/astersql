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

use std::sync::Arc;

use crate::helper::UnmarshalDir;
use crate::memstore::NewMemStorage;
use crate::storage::{Context, Storage, StorageRef, WalkOption};

#[test]
fn unmarshal_dir_stops_after_the_first_worker_error() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(NewMemStorage());
    for index in 0..16 {
        storage
            .WriteFile(&ctx, &format!("meta/{index}.json"), b"not-json")
            .unwrap();
    }

    let results = UnmarshalDir(
        ctx,
        WalkOption {
            sub_dir: "meta/".into(),
            ..WalkOption::default()
        },
        storage,
        |_name, bytes| serde_json::from_slice::<serde_json::Value>(bytes).map_err(Into::into),
    )
    .collect::<Vec<_>>();

    assert_eq!(
        results.iter().filter(|result| result.is_err()).count(),
        1,
        "Go's errgroup reports one terminal error and cancels the remaining workers"
    );
}
