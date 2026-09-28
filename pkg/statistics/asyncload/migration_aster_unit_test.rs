// Copyright 2026 AsterSQL.
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

// `NeededStatsMap` 的迁移单元测试：验证分片映射的插入、升级、删除与并发语义。
//
// `NeededStatsMap` 记录待异步加载的表项（列/索引 ID）；`FullLoad` 只能从 false 升为 true，不可降级。

use super::*;
use std::sync::Arc;

/// 构造测试用的 `TableItemID`（表 ID、列/索引 ID、是否索引、同步加载失败标记）。
fn item(table_id: i64, id: i64, is_index: bool, sync_failed: bool) -> TableItemID {
    TableItemID {
        TableID: table_id,
        ID: id,
        IsIndex: is_index,
        IsSyncLoadFailed: sync_failed,
    }
}

#[test]
/// 新建映射应为空。
fn migration_new_map_starts_empty() {
    let needed = newNeededStatsMap();

    assert_eq!(needed.Length(), 0);
    assert!(needed.AllItems().is_empty());
}

#[test]
/// Insert 必须完整保留 Go 侧键字段与 FullLoad 取值。
fn migration_insert_preserves_the_complete_go_key_and_value() {
    let needed = newNeededStatsMap();
    needed.Insert(item(41, -7, true, true), false);

    let loaded = needed.AllItems();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].TableItemID.TableID, 41);
    assert_eq!(loaded[0].TableItemID.ID, -7);
    assert!(loaded[0].TableItemID.IsIndex);
    assert!(loaded[0].TableItemID.IsSyncLoadFailed);
    assert!(!loaded[0].FullLoad);
}

#[test]
/// FullLoad 只允许升级为 true，后续再 Insert(false) 不得降级。
fn migration_full_load_only_upgrades_and_never_downgrades() {
    let needed = newNeededStatsMap();

    needed.Insert(item(1, 9, false, false), false);
    needed.Insert(item(1, 9, false, false), true);
    needed.Insert(item(1, 9, false, false), false);

    assert_eq!(needed.Length(), 1);
    assert!(needed.AllItems()[0].FullLoad);
}

#[test]
/// IsIndex / IsSyncLoadFailed 等字段均参与 map 身份，不得互相覆盖。
fn migration_all_table_item_fields_participate_in_map_identity() {
    let needed = newNeededStatsMap();

    needed.Insert(item(8, 3, false, false), false);
    needed.Insert(item(8, 3, true, false), false);
    needed.Insert(item(8, 3, false, true), false);

    assert_eq!(needed.Length(), 3);
}

#[test]
/// Delete 只删除精确匹配的项，不影响同表其他项。
fn migration_delete_removes_only_the_exact_item() {
    let needed = newNeededStatsMap();
    needed.Insert(item(12, 5, false, false), false);
    needed.Insert(item(12, 5, true, false), true);

    needed.Delete(item(12, 5, false, false));

    let loaded = needed.AllItems();
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].TableItemID.IsIndex);
    assert!(loaded[0].FullLoad);
}

#[test]
/// 正负 ID 可落在同一分片，但不得因哈希碰撞互相覆盖。
fn migration_negative_and_positive_ids_share_a_shard_without_colliding() {
    let needed = newNeededStatsMap();
    assert_eq!(
        getIdx(&item(1, 17, false, false)),
        getIdx(&item(2, -17, false, false))
    );

    needed.Insert(item(1, 17, false, false), false);
    needed.Insert(item(2, -17, false, false), true);

    assert_eq!(needed.Length(), 2);
}

#[test]
/// 多线程并发 Insert/升级时，条目数与 FullLoad 不得丢失。
fn migration_concurrent_inserts_and_upgrades_are_not_lost() {
    const WORKERS: i64 = 8;
    const ITEMS_PER_WORKER: i64 = 100;
    let needed = Arc::new(newNeededStatsMap());
    let mut workers = Vec::new();

    for worker in 0..WORKERS {
        let needed = Arc::clone(&needed);
        workers.push(std::thread::spawn(move || {
            // 每个 worker 写入互不重叠的 table_id，并强制升级为 FullLoad。
            for id in 0..ITEMS_PER_WORKER {
                let table_id = worker * ITEMS_PER_WORKER + id;
                needed.Insert(item(table_id, id - 50, false, false), false);
                needed.Insert(item(table_id, id - 50, false, false), true);
            }
        }));
    }

    for worker in workers {
        worker.join().expect("concurrent insert worker panicked");
    }

    assert_eq!(needed.Length(), (WORKERS * ITEMS_PER_WORKER) as usize);
    assert!(needed.AllItems().iter().all(|loaded| loaded.FullLoad));
}
