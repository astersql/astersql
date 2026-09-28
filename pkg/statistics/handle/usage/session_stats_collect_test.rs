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

// 会话统计收集的单元测试。
//
// 覆盖 `sweep` 合并会话 delta/usage、移除已删除会话，以及二次 sweep 无残留。

use crate::{
    SessionStatsList, TableDelta, TableDeltaMap, TableItemId, collect_pending_stats_delta_table_ids,
};
use std::collections::HashMap;
use std::time::SystemTime;

/// 验证：会话更新后 delete，sweep 将增量与使用时间并入全局，并清除已删会话。
#[test]
fn canonical_session_sweep_merges_delta_usage_and_removes_deleted_session() {
    let list = SessionStatsList::default();
    let session = list.new_item();
    session.update(7, 3, 4);
    let column = TableItemId {
        table_id: 7,
        id: 9,
        is_index: false,
    };
    session.update_column_usage([column], SystemTime::UNIX_EPOCH);
    session.delete();
    list.sweep();
    let delta = list.table_delta().take();
    assert_eq!((delta[&7].delta, delta[&7].count), (3, 4));
    assert_eq!(list.stats_usage().take()[&column], SystemTime::UNIX_EPOCH);
    // 再次 sweep：已无会话条目，全局也不应再出现新增量
    list.sweep();
    assert!(list.table_delta().take().is_empty());
}

#[test]
fn canonical_table_delta_merge_always_accumulates_equal_values_and_keeps_earliest_time() {
    let map = TableDeltaMap::default();
    let early = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
    let late = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20);
    map.merge(HashMap::from([(
        7,
        TableDelta {
            delta: 2,
            count: 3,
            col_size: 4,
            init_time: Some(late),
        },
    )]));
    map.merge(HashMap::from([(
        7,
        TableDelta {
            delta: 2,
            count: 3,
            col_size: 4,
            init_time: Some(early),
        },
    )]));
    let value = map.take().remove(&7).unwrap();
    assert_eq!((value.delta, value.count, value.col_size), (4, 6, 8));
    assert_eq!(value.init_time, Some(early));

    let equal = TableDelta {
        delta: 1,
        count: 1,
        col_size: 1,
        init_time: Some(early),
    };
    let equal_map = TableDeltaMap::default();
    equal_map.merge(HashMap::from([(8, equal)]));
    equal_map.merge(HashMap::from([(8, equal)]));
    assert_eq!(equal_map.take()[&8].delta, 2);
}

#[test]
fn canonical_pending_delta_ids_are_unique_and_sorted_like_go() {
    let map = HashMap::from([
        (9, TableDelta::default()),
        (2, TableDelta::default()),
        (5, TableDelta::default()),
    ]);
    assert_eq!(
        collect_pending_stats_delta_table_ids(&map, &[5, 5, 9, 4, 2]),
        vec![2, 5, 9]
    );
    assert_eq!(
        collect_pending_stats_delta_table_ids(&map, &[]),
        vec![2, 5, 9]
    );
}

#[test]
#[should_panic(expected = "table ID should be greater than 0")]
fn canonical_table_delta_update_rejects_non_positive_table_ids() {
    TableDeltaMap::default().update(0, 1, 1);
}
