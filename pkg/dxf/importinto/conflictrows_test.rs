// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage::TaskCleanupInfo;
use astersql_objstore::storage::{Context, Storage};

use crate::conflictrows::*;

struct Getter {
    infos: HashMap<i64, TaskCleanupInfo>,
    calls: Mutex<Vec<Vec<i64>>>,
}

impl TaskInfoGetter for Getter {
    fn GetTaskCleanupInfoByIDs(
        &self,
        _ctx: &Context,
        task_ids: &[i64],
    ) -> anyhow::Result<HashMap<i64, TaskCleanupInfo>> {
        self.calls.lock().unwrap().push(task_ids.to_vec());
        Ok(task_ids
            .iter()
            .filter_map(|id| self.infos.get(id).cloned().map(|info| (*id, info)))
            .collect())
    }
}

fn info(
    id: i64,
    task_type: proto::TaskType,
    state: proto::TaskState,
    end: Option<SystemTime>,
) -> TaskCleanupInfo {
    TaskCleanupInfo {
        ID: id,
        Type: task_type,
        State: state,
        EndTime: end,
    }
}

#[test]
fn parses_only_positive_task_ids_with_nonempty_descendants() {
    for (path, expected) in [
        ("conflicted-rows/42/data", Some(42)),
        ("conflicted-rows/007/data", Some(7)),
        ("conflicted-rows/42//data", Some(42)),
        ("other/42/data", None),
        ("conflicted-rows/0/data", None),
        ("conflicted-rows/-1/data", None),
        ("conflicted-rows/+1/data", None),
        ("conflicted-rows/1a/data", None),
        ("conflicted-rows/9223372036854775808/data", None),
        ("conflicted-rows/42///", None),
    ] {
        assert_eq!(super::conflictrows::parse_task_id(path), expected, "{path}");
    }
}

#[test]
fn cleanup_matches_retention_and_invalid_metadata_policy() {
    let ctx = Context::background();
    let store = Arc::new(astersql_objstore::memstore::NewMemStorage());
    let files = [
        "conflicted-rows/1/a",
        "conflicted-rows/2/b",
        "conflicted-rows/3/c",
        "conflicted-rows/4/d",
        "conflicted-rows/bad/e",
        "other/keep",
    ];
    for file in files {
        store.WriteFile(&ctx, file, b"row").unwrap();
    }
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
    let getter = Getter {
        infos: HashMap::from([
            (1, info(1, proto::ImportInto, proto::TaskStateFailed, None)),
            (2, info(2, proto::ImportInto, proto::TaskStateRunning, None)),
            (
                3,
                info(3, proto::TaskTypeExample, proto::TaskStateFailed, None),
            ),
            (
                4,
                info(
                    4,
                    proto::ImportInto,
                    proto::TaskStateSucceed,
                    Some(now - Duration::from_secs(7 * 24 * 60 * 60)),
                ),
            ),
        ]),
        calls: Mutex::new(Vec::new()),
    };

    let stats = CleanFiles(&ctx, store.as_ref(), &getter, now).unwrap();
    assert_eq!(stats.DeletedFiles, 4);
    assert_eq!(stats.NonImportIntoTaskFiles.Count, 1);
    assert_eq!(stats.UnparsedTaskIDFiles.Count, 1);
    assert!(!store.FileExists(&ctx, files[0]).unwrap());
    assert!(store.FileExists(&ctx, files[1]).unwrap());
    assert!(!store.FileExists(&ctx, files[2]).unwrap());
    assert!(!store.FileExists(&ctx, files[3]).unwrap());
    assert!(!store.FileExists(&ctx, files[4]).unwrap());
    assert!(store.FileExists(&ctx, files[5]).unwrap());
    assert_eq!(*getter.calls.lock().unwrap(), vec![vec![1, 2, 3, 4]]);
}

#[test]
fn missing_metadata_and_success_boundary_follow_go_behavior() {
    let ctx = Context::background();
    let store = Arc::new(astersql_objstore::memstore::NewMemStorage());
    for file in [
        "conflicted-rows/10/a",
        "conflicted-rows/11/b",
        "conflicted-rows/12/c",
    ] {
        store.WriteFile(&ctx, file, b"row").unwrap();
    }
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
    let getter = Getter {
        infos: HashMap::from([
            (
                11,
                info(
                    11,
                    proto::ImportInto,
                    proto::TaskStateSucceed,
                    Some(now - super::conflictrows::RETENTION),
                ),
            ),
            (
                12,
                info(
                    12,
                    proto::ImportInto,
                    proto::TaskStateSucceed,
                    Some(now - super::conflictrows::RETENTION + Duration::from_nanos(1)),
                ),
            ),
        ]),
        calls: Mutex::new(Vec::new()),
    };

    let stats = CleanFiles(&ctx, store.as_ref(), &getter, now).unwrap();
    assert_eq!(stats.DeletedFiles, 2);
    assert_eq!(stats.MissingTasks.Count, 1);
    assert_eq!(stats.MissingTaskFiles.Count, 1);
    assert!(!store.FileExists(&ctx, "conflicted-rows/10/a").unwrap());
    assert!(!store.FileExists(&ctx, "conflicted-rows/11/b").unwrap());
    assert!(store.FileExists(&ctx, "conflicted-rows/12/c").unwrap());
}

#[test]
fn generated_prefix_keeps_conflict_rows_outside_task_temp_directory() {
    let prefix = NewFileNamePrefixWithUUID(12, 34, "abcd");
    assert_eq!(prefix, "conflicted-rows/12/34-abcd");
    assert!(NewFileNamePrefix(12, 34).starts_with("conflicted-rows/12/34-"));
}

#[test]
fn empty_cleanup_stats_serialize_as_an_empty_object() {
    assert_eq!(
        serde_json::to_string(&CleanupStats::default()).unwrap(),
        "{}"
    );
}

#[test]
fn task_batch_soft_limit_keeps_the_current_walk_callback() {
    let ctx = Context::background();
    let store = Arc::new(astersql_objstore::memstore::NewMemStorage());
    let mut infos = HashMap::new();
    for task_id in 1..=129 {
        store
            .WriteFile(&ctx, &format!("conflicted-rows/{task_id:03}/data"), b"row")
            .unwrap();
        infos.insert(
            task_id,
            info(task_id, proto::ImportInto, proto::TaskStateFailed, None),
        );
    }
    let getter = Getter {
        infos,
        calls: Mutex::new(Vec::new()),
    };

    let stats = CleanFiles(&ctx, store.as_ref(), &getter, SystemTime::now()).unwrap();
    assert_eq!(stats.DeletedFiles, 129);
    assert_eq!(getter.calls.lock().unwrap()[0].len(), 129);
}

#[test]
fn cancelled_walk_returns_failure_stats_without_metadata_lookup() {
    let ctx = Context::background();
    let store = Arc::new(astersql_objstore::memstore::NewMemStorage());
    store
        .WriteFile(&ctx, "conflicted-rows/1/data", b"row")
        .unwrap();
    ctx.cancel();
    let getter = Getter {
        infos: HashMap::new(),
        calls: Mutex::new(Vec::new()),
    };

    let failure = CleanFiles(&ctx, store.as_ref(), &getter, SystemTime::now()).unwrap_err();
    assert_eq!(failure.Stats.Failures, 1);
    assert!(failure.Source.to_string().contains("context canceled"));
    assert!(getter.calls.lock().unwrap().is_empty());
}
